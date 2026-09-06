import { fromMarkdown } from 'mdast-util-from-markdown';

export type SourceRange = { start: number; end: number };
export type Replacement = SourceRange & { value: string };
type PositionedNode = {
  type: string;
  position?: { start: { offset?: number }; end: { offset?: number } };
  children?: PositionedNode[];
};

// Use CommonMark positions rather than a backtick regex: nested, indented,
// unclosed and variable-length fences must remain literal code examples.
export function protectedRanges(source: string): SourceRange[] {
  const ranges: SourceRange[] = [];
  const visit = (node: PositionedNode) => {
    if (['code', 'inlineCode', 'html', 'link', 'image', 'definition'].includes(node.type)) {
      const start = node.position?.start.offset;
      const end = node.position?.end.offset;
      if (start !== undefined && end !== undefined) ranges.push({ start, end });
      return;
    }
    node.children?.forEach(visit);
  };
  visit(fromMarkdown(source));
  return ranges.sort((a, b) => a.start - b.start);
}

export function isProtected(offset: number, ranges: SourceRange[]): boolean {
  let low = 0;
  let high = ranges.length - 1;
  while (low <= high) {
    const middle = (low + high) >>> 1;
    const range = ranges[middle];
    if (offset < range.start) high = middle - 1;
    else if (offset >= range.end) low = middle + 1;
    else return true;
  }
  return false;
}

export function isEscaped(source: string, offset: number): boolean {
  let slashes = 0;
  while (offset > 0 && source[--offset] === '\\') slashes++;
  return slashes % 2 === 1;
}

export function replaceRanges(source: string, replacements: Replacement[]): string {
  let cursor = 0;
  const output: string[] = [];
  for (const replacement of replacements.sort((a, b) => a.start - b.start)) {
    if (replacement.start < cursor) continue;
    output.push(source.slice(cursor, replacement.start), replacement.value);
    cursor = replacement.end;
  }
  output.push(source.slice(cursor));
  return output.join('');
}

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]!);
}

// Encoded punctuation stays text even when generated labels pass through Markdown.
export function literalText(value: string): string {
  return escapeHtml(value).replace(/[\\`*_{}[\]()#!|~$]/g, (char) => `&#${char.charCodeAt(0)};`);
}
