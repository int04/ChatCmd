import assert from "node:assert/strict";
import test from "node:test";

const before = (total, count) => total / count;
const after = (total, count) => (count === 0 ? undefined : total / count);

test("regression fails before and passes after without weakening the assertion", () => {
  let failedBefore = false;
  try {
    assert.equal(before(10, 0), undefined);
  } catch {
    failedBefore = true;
  }
  assert.equal(failedBefore, true);
  assert.equal(after(10, 0), undefined);
});

