export function checkedAdd(left, right) {
  if (!Number.isSafeInteger(left) || !Number.isSafeInteger(right)) return undefined;
  const sum = left + right;
  return Number.isSafeInteger(sum) ? sum : undefined;
}

