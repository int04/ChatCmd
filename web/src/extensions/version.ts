export function isExtensionVersionOutdated(detected: string | undefined, required: string): boolean {
  const parse = (value: string | undefined) => value && /^\d+(?:\.\d+)*$/.test(value)
    ? value.split('.').map(Number) : null;
  const current = parse(detected);
  const minimum = parse(required);
  if (!current || !minimum) return false;
  for (let index = 0; index < Math.max(current.length, minimum.length); index += 1) {
    if ((current[index] ?? 0) !== (minimum[index] ?? 0)) {
      return (current[index] ?? 0) < (minimum[index] ?? 0);
    }
  }
  return false;
}
