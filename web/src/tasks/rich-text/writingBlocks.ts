import { isEscaped, isProtected, protectedRanges } from './sourceRanges';

export type WritingPart = {
  kind: 'writing'; start: number; content: string; attributes: Record<string, string>; closed: boolean;
};
export type RichTextPart = { kind: 'markdown'; start: number; content: string } | WritingPart;

function readHeader(source: string, brace: number) {
  let quote = '';
  let escaped = false;
  for (let index = brace + 1; index < Math.min(source.length, brace + 8192); index++) {
    const char = source[index];
    if (escaped) { escaped = false; continue; }
    if (char === '\\' && quote) { escaped = true; continue; }
    if (quote) { if (char === quote) quote = ''; continue; }
    if (char === '"' || char === "'") { quote = char; continue; }
    if (char === '}') return index + 1;
  }
  return null;
}

function readAttributes(value: string): Record<string, string> | null {
  const attributes: Record<string, string> = Object.create(null);
  const pattern = /\s*([a-z][\w-]*)\s*=\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^\s"'={}]+))/giy;
  let cursor = 0;
  while (cursor < value.length && value.slice(cursor).trim()) {
    pattern.lastIndex = cursor;
    const match = pattern.exec(value);
    if (!match) return null;
    const key = match[1].toLowerCase();
    attributes[key] = (match[2] ?? match[3] ?? match[4]).replace(/\\([\\"'])/g, '$1');
    cursor = pattern.lastIndex;
  }
  return attributes;
}

/** Split only recognized writing envelopes; the original event text is never changed. */
export function splitWritingBlocks(source: string): RichTextPart[] {
  if (!source.includes(':::writing')) return [{ kind: 'markdown', start: 0, content: source }];
  const ranges = protectedRanges(source);
  const bbCode = [...source.matchAll(/\[(code|pre)(?:=[^\]\r\n]*)?\][\s\S]*?\[\/\1\]/gi)]
    .filter((match) => !isProtected(match.index, ranges) && !isEscaped(source, match.index))
    .map((match) => ({ start: match.index, end: match.index + match[0].length }));
  const protectedAt = (offset: number) => isProtected(offset, ranges)
    || bbCode.some((range) => offset >= range.start && offset < range.end);
  const openings: { start: number; end: number; attributes: Record<string, string> }[] = [];
  for (const match of source.matchAll(/:::writing[ \t]*\{/g)) {
    if (match.index < (openings.at(-1)?.end ?? 0) || protectedAt(match.index) || isEscaped(source, match.index)
      || (match.index > 0 && !/\s/.test(source[match.index - 1]))) continue;
    const brace = match.index + match[0].length - 1;
    const end = readHeader(source, brace);
    if (end === null) continue;
    const attributes = readAttributes(source.slice(brace + 1, end - 1));
    if (attributes) openings.push({ start: match.index, end, attributes });
  }
  if (!openings.length) return [{ kind: 'markdown', start: 0, content: source }];
  const closings = [...source.matchAll(/:::(?=\s|$)/g)]
    .filter((match) => !protectedAt(match.index) && !isEscaped(source, match.index)
      && (match.index === 0 || /\s/.test(source[match.index - 1])))
    .map((match) => match.index);
  const parts: RichTextPart[] = [];
  let cursor = 0;
  let closingIndex = 0;
  for (let index = 0; index < openings.length; index++) {
    const opening = openings[index];
    if (opening.start < cursor) continue;
    if (opening.start > cursor) parts.push({ kind: 'markdown', start: cursor, content: source.slice(cursor, opening.start) });
    const boundary = openings[index + 1]?.start ?? source.length;
    while (closingIndex < closings.length && closings[closingIndex] < opening.end) closingIndex++;
    const closing = closings[closingIndex];
    const closed = closing !== undefined && closing < boundary;
    const end = closed ? closing : boundary;
    parts.push({ kind: 'writing', start: opening.start, attributes: opening.attributes,
      content: source.slice(opening.end, end).trim(), closed });
    cursor = closed ? end + 3 : end;
  }
  if (cursor < source.length) parts.push({ kind: 'markdown', start: cursor, content: source.slice(cursor) });
  return parts;
}
