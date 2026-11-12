# Shared project checks

Consumer repositories keep project data in `.project/verification/checks.json`.
The CLI implements the validation so consumers do not duplicate Python engines.
All paths below are repository-relative; traversal and symlink inputs are rejected.

```json
{
  "screenshots": {
    "library": {
      "directory": "library/src/screenshotTestDebug/reference",
      "scenarios": ["front", "back"],
      "metadata": {"renderer": "Compose Preview Screenshot Testing", "kind": "host-rendered synthetic UI"}
    }
  },
  "junit": {
    "capture": {
      "directory": "library/build/outputs/androidTest-results/connected/debug",
      "required": [["CaptureTest", "frontIsVisible"]]
    }
  },
  "release_validation": [["android-repo-setup", "build_docs", "--check"]]
}
```

- `project-checks screenshots --module library` requires exactly one PNG filename containing `_scenario_` per configured scenario, rejects unexpected or ambiguous files, validates PNG header/dimensions, and writes `render-manifest.json` with source commit and byte hashes. This records candidates, not approval or device evidence. Update renderer metadata alongside renderer dependency versions.
- `project-checks junit --suite capture` requires every named testcase in `TEST-*.xml` reports to pass. Missing, failed, errored, or skipped required tests fail. Fully qualified classes match exactly; simple class names match the final segment for compatibility with Android reports.
- `release-validate --source <full-SHA>` checks out that commit into a temporary detached worktree, reads its committed `release_validation` argv steps, runs them in order, and removes the worktree on success or failure. Commands are trusted repository code and may require Node, npm, Zola or other project tools. This preserves separation from the release artifact checkout; it is not a sandbox. Historical documentation alone is never executed by the documentation-history reader.

Only declarative inputs belong in this file. Domain-specific documentation generators and manual media acceptance policies can remain in a consumer's scripts directory.
