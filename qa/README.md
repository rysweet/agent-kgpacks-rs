# QA scenarios (gadugi-test)

`gadugi-test` (`@gadugi/agentic-test`) CLI scenarios that exercise the WS1 CVE
eval surface (issue #16) through the actual command surface. Each scenario runs a
real command; a non-zero exit fails the scenario.

Run from the repository root (so commands resolve against the workspace):

```bash
# Validate the scenario files:
gadugi-test validate -d qa/scenarios

# Run them:
gadugi-test run -d qa/scenarios
```

| Scenario                          | Validates                                                        |
| --------------------------------- | ---------------------------------------------------------------- |
| `eval-question-schema.yaml`       | The committed CVE eval questions meet the schema acceptance.     |
| `full-pack-eval-recall.yaml`      | Full-pack (unsampled) recall@k matches the committed artifact.   |
| `eval-harness-offline-suite.yaml` | The whole `kgpacks-eval` harness passes offline.                 |
