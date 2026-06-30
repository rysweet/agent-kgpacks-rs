//! LLM-based knowledge extraction (schema, sanitization, normalization).
//!
//! Rust port of `bootstrap/src/extraction/llm_extractor.py`. The actual model
//! call is gated behind the [`Extractor`] trait so the pipeline runs
//! hermetically in CI: tests use [`MockExtractor`] (a canned result) or
//! [`JsonExtractor`] (parses a fixed JSON string through the real
//! sanitization path). A live Claude/Copilot backend can implement [`Extractor`]
//! later without touching the pipeline.
//!
//! The *pure* logic — response sanitization (`SEC-08`), relation normalization,
//! and domain detection — is ported directly and is the subject of the
//! `extractor schema validation` parity test.

use std::collections::BTreeMap;
use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::content::ParsedSection;

/// The canonical relation vocabulary (parity with `STANDARD_RELATIONS`).
pub const STANDARD_RELATIONS: [&str; 21] = [
    "founded",
    "invented",
    "discovered",
    "developed",
    "created",
    "led",
    "directed",
    "authored",
    "influenced",
    "inspired",
    "part_of",
    "uses",
    "requires",
    "caused",
    "resulted_in",
    "fought_in",
    "participated_in",
    "born_in",
    "died_in",
    "located_in",
    "related_to",
];

/// Map a normalized relation synonym to its canonical form (parity with
/// `_RELATION_SYNONYMS`). Returns `None` if there is no known synonym.
fn relation_synonym(normalized: &str) -> Option<&'static str> {
    let canonical = match normalized {
        "established" | "co_founded" | "cofounded" | "set_up" => "founded",
        "built" | "made" | "constructed" | "designed" => "created",
        "devised" | "conceived" | "patented" => "invented",
        "found" | "uncovered" | "identified" => "discovered",
        "built_on" | "advanced" | "improved" | "refined" => "developed",
        "headed" | "managed" | "chaired" | "ran" => "led",
        "supervised" | "oversaw" => "directed",
        "wrote" | "published" => "authored",
        "affected" | "impacted" | "shaped" => "influenced",
        "motivated" => "inspired",
        "component_of" | "member_of" | "belongs_to" | "subset_of" => "part_of",
        "employs" | "utilizes" => "uses",
        "relies_on" | "depends_on" | "needs" => "requires",
        "led_to" | "triggered" => "caused",
        "produced" | "generated" => "resulted_in",
        "battled_in" => "fought_in",
        "served_in" | "engaged_in" | "took_part_in" => "participated_in",
        _ => return None,
    };
    Some(canonical)
}

/// Normalize a relation string to a canonical form.
///
/// Lowercases, replaces spaces and hyphens with underscores, then maps known
/// synonyms. Unknown relations are kept as-is. Parity with `normalize_relation`.
///
/// Note: `"co-founded"` normalizes (hyphen → underscore) to `"co_founded"`,
/// which the synonym table maps to `"founded"`.
pub fn normalize_relation(relation: &str) -> String {
    let normalized = relation.trim().to_lowercase().replace([' ', '-'], "_");
    if STANDARD_RELATIONS.contains(&normalized.as_str()) {
        return normalized;
    }
    relation_synonym(&normalized)
        .map(str::to_string)
        .unwrap_or(normalized)
}

/// Domain keyword tables (parity with `_DOMAIN_KEYWORDS`), in priority order so
/// ties resolve to the earlier domain.
const DOMAIN_KEYWORDS: [(&str, &[&str]); 4] = [
    (
        "history",
        &[
            "history",
            "war",
            "battle",
            "revolution",
            "empire",
            "dynasty",
            "political",
            "government",
            "military",
            "colonial",
            "medieval",
        ],
    ),
    (
        "science",
        &[
            "physics",
            "chemistry",
            "biology",
            "mathematics",
            "computer",
            "engineering",
            "technology",
            "algorithm",
            "quantum",
            "molecular",
        ],
    ),
    (
        "biography",
        &[
            "people",
            "person",
            "biography",
            "leader",
            "president",
            "scientist",
            "artist",
            "writer",
            "philosopher",
            "musician",
        ],
    ),
    (
        "geography",
        &[
            "country",
            "city",
            "region",
            "continent",
            "geography",
            "river",
            "mountain",
            "island",
            "ocean",
            "state",
        ],
    ),
];

/// Classify an article's domain from its categories.
///
/// Uses `\w`-delimited token matching: Python's reference matches `\bkeyword\b`,
/// and since `_` is a word character in Python regex, underscores stay *within*
/// a token here too (so `"World_War_II"` is one token, not `war`). Requires at
/// least two keyword hits to avoid single-keyword misclassification. Returns the
/// best-scoring domain, or `None`. Parity with `detect_domain`.
pub fn detect_domain(categories: &[String]) -> Option<String> {
    if categories.is_empty() {
        return None;
    }
    let tokens: HashSet<String> = categories
        .iter()
        .flat_map(|c| {
            c.to_lowercase()
                .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();

    let mut best_domain: Option<&str> = None;
    let mut best_score = 0usize;
    for (domain, keywords) in DOMAIN_KEYWORDS {
        let score = keywords.iter().filter(|kw| tokens.contains(**kw)).count();
        if score > best_score {
            best_score = score;
            best_domain = Some(domain);
        }
    }
    if best_score >= 2 {
        best_domain.map(str::to_string)
    } else {
        None
    }
}

/// Validate and filter the `entities` list from an LLM response (SEC-08).
///
/// Drops entries with a missing/empty/non-string `name`, truncates a `name`
/// longer than 256 characters, and defaults a missing/empty `type` to
/// `"concept"`. Non-object elements are skipped; a non-array input yields an
/// empty list. Parity with `_sanitize_entities`.
pub fn sanitize_entities(raw: &Value) -> Vec<Value> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    let mut valid = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let name = match obj.get("name").and_then(Value::as_str) {
            Some(n) if !n.trim().is_empty() => n,
            _ => continue,
        };
        let mut cleaned: Map<String, Value> = obj.clone();
        if name.chars().count() > 256 {
            let truncated: String = name.chars().take(256).collect();
            cleaned.insert("name".to_string(), Value::String(truncated));
        }
        let type_ok = obj
            .get("type")
            .and_then(Value::as_str)
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        if !type_ok {
            cleaned.insert("type".to_string(), Value::String("concept".to_string()));
        }
        valid.push(Value::Object(cleaned));
    }
    valid
}

/// Validate and filter the `relationships` list from an LLM response (SEC-08).
///
/// Drops entries where `source`, `target`, or `relation` is missing, empty, or
/// non-string. Non-object elements are skipped; a non-array input yields an
/// empty list. Parity with `_sanitize_relationships`.
pub fn sanitize_relationships(raw: &Value) -> Vec<Value> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    let mut valid = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let ok = ["source", "target", "relation"].iter().all(|field| {
            obj.get(*field)
                .and_then(Value::as_str)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
        });
        if ok {
            valid.push(Value::Object(obj.clone()));
        }
    }
    valid
}

/// Validate and filter the `key_facts` list from an LLM response (SEC-08).
///
/// Drops non-string and whitespace-only elements, strips surrounding
/// whitespace, and truncates a fact longer than 1024 characters. A non-array
/// input yields an empty list. Parity with `_sanitize_key_facts`.
pub fn sanitize_key_facts(raw: &Value) -> Vec<String> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    let mut valid = Vec::new();
    for item in items {
        let Some(fact) = item.as_str() else {
            continue;
        };
        let fact = fact.trim();
        if fact.is_empty() {
            continue;
        }
        let truncated: String = fact.chars().take(1024).collect();
        valid.push(truncated);
    }
    valid
}

/// An extracted entity (parity with the reference `Entity` dataclass).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    /// Entity name.
    pub name: String,
    /// Entity type (`person`, `place`, `organization`, `concept`, `event`, …).
    pub type_: String,
    /// Free-form key/value properties.
    pub properties: BTreeMap<String, Value>,
}

/// A relationship between two entities (parity with the reference
/// `Relationship` dataclass).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    /// Source entity name.
    pub source: String,
    /// Normalized relation verb.
    pub relation: String,
    /// Target entity name.
    pub target: String,
    /// Sentence/clause where the relationship appears.
    pub context: String,
}

/// A complete extraction from an article (parity with `ExtractionResult`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractionResult {
    /// Extracted entities.
    pub entities: Vec<Entity>,
    /// Extracted relationships.
    pub relationships: Vec<Relationship>,
    /// Extracted key facts.
    pub key_facts: Vec<String>,
}

/// Parse and sanitize a raw LLM JSON response into an [`ExtractionResult`].
///
/// Mirrors the response-handling half of `extract_from_article`: strips a
/// Markdown code fence, narrows to the first `{ … }` object, parses JSON, and
/// runs every value through the SEC-08 sanitizers. A response that cannot be
/// parsed as JSON yields an empty result (the reference's `JSONDecodeError`
/// fallback), never an error.
pub fn parse_extraction_response(content: &str) -> ExtractionResult {
    let json_text = extract_json_object(content);
    let Ok(data) = serde_json::from_str::<Value>(&json_text) else {
        return ExtractionResult::default();
    };

    let empty = Value::Array(Vec::new());
    let entities = sanitize_entities(data.get("entities").unwrap_or(&empty))
        .into_iter()
        .map(|e| Entity {
            name: e
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            type_: e
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("concept")
                .to_string(),
            properties: e
                .get("properties")
                .and_then(Value::as_object)
                .map(|m| m.clone().into_iter().collect())
                .unwrap_or_default(),
        })
        .collect();

    let relationships = sanitize_relationships(data.get("relationships").unwrap_or(&empty))
        .into_iter()
        .map(|r| Relationship {
            source: r
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            relation: normalize_relation(
                r.get("relation")
                    .and_then(Value::as_str)
                    .unwrap_or("related_to"),
            ),
            target: r
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            context: r
                .get("context")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect();

    let key_facts = sanitize_key_facts(data.get("key_facts").unwrap_or(&empty));

    ExtractionResult {
        entities,
        relationships,
        key_facts,
    }
}

/// Narrow raw LLM `content` to its JSON object: strip a Markdown code fence if
/// present, then take the span from the first `{` to the last `}` unless the
/// text is already a bare object. Parity with the fence/boundary stripping in
/// `extract_from_article` (the `{…}` narrowing always runs after fence stripping,
/// so prose inside a fence is still tolerated).
fn extract_json_object(content: &str) -> String {
    let mut text = content.trim().to_string();

    // Strip a Markdown code fence if present.
    if let Some((_, after)) = text.split_once("```json") {
        if let Some((body, _)) = after.split_once("```") {
            text = body.trim().to_string();
        }
    } else if let Some((_, rest)) = text.split_once("```") {
        if let Some((body, _)) = rest.split_once("```") {
            text = body.trim().to_string();
        }
    }

    // Narrow to the first `{` … last `}` unless the text already starts with `{`.
    if !text.trim_start().starts_with('{') {
        if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
            if end > start {
                text = text[start..=end].to_string();
            }
        }
    }
    text.trim().to_string()
}

/// LLM extractor abstraction. The pipeline holds a `&dyn Extractor`, so the
/// model call is fully mockable (parity with the reference `LLMExtractor`,
/// minus the network).
pub trait Extractor {
    /// Extract entities, relationships, and key facts from an article.
    fn extract_from_article(
        &self,
        title: &str,
        sections: &[ParsedSection],
        max_sections: usize,
        domain: Option<&str>,
    ) -> ExtractionResult;
}

/// An [`Extractor`] that returns a fixed, canned [`ExtractionResult`].
#[derive(Debug, Clone, Default)]
pub struct MockExtractor {
    /// The result returned for every article.
    pub result: ExtractionResult,
}

impl MockExtractor {
    /// Create a mock extractor returning `result`.
    pub fn new(result: ExtractionResult) -> Self {
        Self { result }
    }
}

impl Extractor for MockExtractor {
    fn extract_from_article(
        &self,
        _title: &str,
        _sections: &[ParsedSection],
        _max_sections: usize,
        _domain: Option<&str>,
    ) -> ExtractionResult {
        self.result.clone()
    }
}

/// An [`Extractor`] that runs a caller-supplied "model" (a closure mapping the
/// built prompt to a raw JSON response) through the real
/// [`parse_extraction_response`] sanitization path.
///
/// This exercises the genuine response-handling and SEC-08 sanitization in
/// tests without any network access — stand in for a real model by returning a
/// JSON string.
pub struct JsonExtractor<F>
where
    F: Fn(&str) -> String,
{
    model: F,
    max_chars: usize,
}

impl<F> JsonExtractor<F>
where
    F: Fn(&str) -> String,
{
    /// Wrap `model`, which maps the extraction prompt to a raw JSON response.
    pub fn new(model: F) -> Self {
        Self {
            model,
            max_chars: 8000,
        }
    }
}

impl<F> Extractor for JsonExtractor<F>
where
    F: Fn(&str) -> String,
{
    fn extract_from_article(
        &self,
        title: &str,
        sections: &[ParsedSection],
        max_sections: usize,
        domain: Option<&str>,
    ) -> ExtractionResult {
        let prompt = build_extraction_prompt(title, sections, max_sections, domain, self.max_chars);
        let response = (self.model)(&prompt);
        parse_extraction_response(&response)
    }
}

/// Build the extraction prompt for an article (parity with the prompt-assembly
/// half of `extract_from_article`): combine the first `max_sections` sections,
/// truncate to `max_chars`, and append the domain-specific focus suffix.
pub fn build_extraction_prompt(
    title: &str,
    sections: &[ParsedSection],
    max_sections: usize,
    domain: Option<&str>,
    max_chars: usize,
) -> String {
    let mut combined = format!("# {title}\n\n");
    for section in sections.iter().take(max_sections) {
        if section.title.is_empty() {
            combined.push_str(&format!("{}\n\n", section.content));
        } else {
            combined.push_str(&format!("## {}\n{}\n\n", section.title, section.content));
        }
    }
    if combined.chars().count() > max_chars {
        let truncated: String = combined.chars().take(max_chars).collect();
        combined = format!("{truncated}...[truncated]");
    }

    let mut prompt = format!(
        "Extract structured knowledge from this Wikipedia article.\n\n\
         Article text:\n{combined}\n\n\
         Extract:\n\
         1. **Entities**: Named entities with their type \
         (person/place/organization/concept/event)\n\
         2. **Relationships**: Connections between entities\n\
         3. **Key Facts**: 3-5 most important facts about the main topic\n\n\
         Return JSON with `entities`, `relationships`, and `key_facts`."
    );
    if let Some(suffix) = domain.and_then(domain_prompt) {
        prompt.push_str(suffix);
    }
    prompt
}

/// Domain-specific focus suffix appended to the extraction prompt (parity with
/// `_DOMAIN_PROMPTS`). Returns `None` for an unknown domain.
fn domain_prompt(domain: &str) -> Option<&'static str> {
    let suffix = match domain {
        "history" => {
            "\n\nFocus especially on: causal relationships (what led to what), \
             chronological sequences (before/after/during), key figures and their roles, \
             alliances and conflicts between groups, and turning points."
        }
        "science" => {
            "\n\nFocus especially on: taxonomic/hierarchical relationships (X is a type of Y), \
             inventions and discoveries (who invented/discovered what, when), \
             dependencies (X requires/uses Y), and experimental findings."
        }
        "biography" => {
            "\n\nFocus especially on: life events (born, died, educated at), \
             achievements and contributions, institutional affiliations, \
             influences (who influenced whom), and notable works or creations."
        }
        "geography" => {
            "\n\nFocus especially on: spatial relationships (located in, borders, contains), \
             demographic facts (population, language, government type), \
             natural features, and economic/cultural significance."
        }
        _ => return None,
    };
    Some(suffix)
}
