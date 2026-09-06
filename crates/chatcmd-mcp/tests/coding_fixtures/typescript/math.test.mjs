import assert from "node:assert/strict";
import test from "node:test";
import { checkedAdd } from "./math.mjs";

test("adds safe integers", () => {
  assert.equal(checkedAdd(20, 22), 42);
});

