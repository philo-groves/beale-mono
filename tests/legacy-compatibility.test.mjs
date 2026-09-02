import assert from "node:assert/strict";
import test from "node:test";

import {
  adoptPreBealeRecordKeys,
  installPreBealeEnvironmentAliases,
  preBealeEnvironmentName,
  preBealeRuntimeId,
  readCompatibleRecordValue,
  readCompatibleEnvironment,
} from "../packages/research-agent/dist/index.js";

test("pre-Beale environment overrides remain compatible with app-server names", () => {
  const canonicalName = "BEALE_APP_SERVER_COMMAND";
  const previousName = preBealeEnvironmentName(canonicalName);
  const environment = { [previousName]: "legacy-wrapper" };

  assert.equal(readCompatibleEnvironment(canonicalName, environment), "legacy-wrapper");
  installPreBealeEnvironmentAliases(environment);
  assert.equal(environment[canonicalName], "legacy-wrapper");
});

test("pre-Beale persisted record keys are adopted without changing values", () => {
  const previousKey = `${preBealeRuntimeId()}Kind`;
  const record = { [previousKey]: "tool.observed", nested: [{ [previousKey]: "error.observed" }] };

  assert.equal(readCompatibleRecordValue(record, "appServerKind"), "tool.observed");
  assert.deepEqual(adoptPreBealeRecordKeys(record), {
    appServerKind: "tool.observed",
    nested: [{ appServerKind: "error.observed" }],
  });
});
