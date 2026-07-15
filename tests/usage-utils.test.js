import test from "node:test";
import assert from "node:assert/strict";

import { isPlausibleProviderKey } from "../src/usage-utils.js";

test("provider key validation follows each official key shape", () => {
  assert.equal(isPlausibleProviderKey("minimax", "sk-cp-12345678901234"), true);
  assert.equal(isPlausibleProviderKey("deepseek", "sk-12345678901234567"), true);
  assert.equal(isPlausibleProviderKey("zhipu", "1234567890.abcdefghijkl"), true);
  assert.equal(isPlausibleProviderKey("zhipu", "1234567890 abcdefghijkl"), false);
  assert.equal(isPlausibleProviderKey("unknown", "sk-12345678901234567"), false);
});
