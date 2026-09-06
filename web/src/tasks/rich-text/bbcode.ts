import { escapeHtml, isEscaped, isProtected, literalText, protectedRanges, replaceRanges, type Replacement } from './sourceRanges';

type Tag = { name: string; argument: string; start: number; end: number; closing: boolean };
const tagPattern = /\[(\/?)(b|strong|i|em|u|s|strike|del|sub|sup|kbd|mark|quote|url|email|img|list|ul|ol|li|table|tr|th|td|spoiler|details|center|left|right|justify|color|size|font|h[1-6])(?:=([^\]\r\n]{0,300}))?\]/gi;
const htmlTags: Record<string, string> = { u: 'u', sub: 'sub', sup: 'sup', kbd: 'kbd', mark: 'mark', ul: 'ul', ol: 'ol', li: 'li', table: 'table', tr: 'tr', th: 'th', td: 'td' };
const inlineTags: Record<string, string> = { b: '**', strong: '**', i: '*', em: '*', s: '~~', strike: '~~', del: '~~' };

function unquote(value: string): string {
  const trimmed = value.trim();
  return /^("[\s\S]*"|'[\s\S]*')$/.test(trimmed) ? trimmed.slice(1, -1) : trimmed;
}

function codeBlocks(source: string): string {
  const ranges = protectedRanges(source);
  const replacements: Replacement[] = [];
  const pattern = /\[(code|pre)(?:=([^\]\r\n]*))?\]/gi;
  let consumed = 0;
  for (const match of source.matchAll(pattern)) {
    if (match.index < consumed || isProtected(match.index, ranges) || isEscaped(source, match.index)) continue;
    const start = match.index + match[0].length;
    const close = new RegExp(`\\[/${match[1]}\\]`, 'gi');
    close.lastIndex = start;
    const ending = close.exec(source);
    if (!ending) continue; // Incomplete/unknown syntax stays visible, never deleted.
    const code = source.slice(start, ending.index).replace(/^\r?\n/, '').replace(/\r?\n$/, '');
    let longestFence = 2;
    for (const token of code.matchAll(/`+/g)) longestFence = Math.max(longestFence, token[0].length);
    const fence = '`'.repeat(longestFence + 1);
    const language = unquote(match[2] ?? '');
    const safeLanguage = /^[\w+-]{1,40}$/.test(language) ? language : '';
    consumed = ending.index + ending[0].length;
    replacements.push({ start: match.index, end: consumed, value: `\n\n${fence}${safeLanguage}\n${code}\n${fence}\n\n` });
  }
  return replaceRanges(source, replacements);
}

function wrapInline(marker: string, body: string): string {
  if (!body.trim()) return body;
  const leading = body.match(/^\s*/)?.[0] ?? '';
  const trailing = body.match(/\s*$/)?.[0] ?? '';
  return `${leading}${marker}${body.trim()}${marker}${trailing}`;
}

function listBody(body: string, ordered: boolean): string {
  const items = body.split(/\[\*\]/).map((item) => item.trim()).filter(Boolean);
  if (!body.includes('[*]')) return `<${ordered ? 'ol' : 'ul'}>${body}</${ordered ? 'ol' : 'ul'}>`;
  return '\n\n' + items.map((item, index) => {
    const prefix = ordered ? `${index + 1}. ` : '- ';
    return prefix + item.replace(/\n/g, '\n' + ' '.repeat(prefix.length));
  }).join('\n') + '\n\n';
}

function styledBody(tag: Tag, body: string): string {
  const value = unquote(tag.argument).toLowerCase();
  let className = '';
  if (tag.name === 'color' && /^(red|orange|yellow|green|blue|purple|gray|grey)$/.test(value)) className = `chat-color-${value === 'grey' ? 'gray' : value}`;
  if (tag.name === 'size' && /^[1-7]$/.test(value)) className = `chat-size-${value}`;
  if (tag.name === 'font' && /^(monospace|serif|sans-serif)$/.test(value)) className = `chat-font-${value}`;
  // Arbitrary CSS/fonts are deliberately not trusted. Keep the authored text.
  return className ? `<span class="${className}">${body}</span>` : body;
}

function renderTag(tag: Tag, body: string): string {
  if (inlineTags[tag.name]) return wrapInline(inlineTags[tag.name], body);
  if (htmlTags[tag.name]) return `<${htmlTags[tag.name]}>${body}</${htmlTags[tag.name]}>`;
  if (/^h[1-6]$/.test(tag.name)) return `\n\n${'#'.repeat(Number(tag.name[1]))} ${body.trim()}\n\n`;
  const argument = unquote(tag.argument);
  switch (tag.name) {
    case 'quote': {
      const title = argument ? `**${literalText(argument)}**\n\n` : '';
      return '\n\n' + (title + body.trim()).replace(/^/gm, '> ') + '\n\n';
    }
    case 'url': case 'email': {
      const target = argument || body.trim();
      const href = tag.name === 'email' && !target.startsWith('mailto:') ? `mailto:${target}` : target;
      return `<a href="${escapeHtml(href)}">${argument ? body : literalText(body)}</a>`;
    }
    case 'img': return `<img src="${escapeHtml(body.trim())}" alt="${escapeHtml(argument)}">`;
    case 'list': return listBody(body, /^(1|a|i)$/i.test(argument));
    case 'details': case 'spoiler': return `\n\n<details><summary>${literalText(argument || '…')}</summary>\n\n${body.trim()}\n\n</details>\n\n`;
    case 'left': case 'right': case 'center': case 'justify': return `<div align="${tag.name}">\n\n${body.trim()}\n\n</div>`;
    case 'color': case 'size': case 'font': return styledBody(tag, body);
    default: return body;
  }
}

function pairedTags(source: string, depth: number): string {
  if (depth > 12 || !source.includes('[')) return source;
  const ranges = protectedRanges(source);
  const stack: Tag[] = [];
  const pairs = new Map<number, { opening: Tag; closing: Tag }>();
  for (const match of source.matchAll(tagPattern)) {
    if (isProtected(match.index, ranges) || isEscaped(source, match.index)) continue;
    const tag: Tag = { name: match[2].toLowerCase(), argument: match[3] ?? '', start: match.index,
      end: match.index + match[0].length, closing: Boolean(match[1]) };
    if (!tag.closing) stack.push(tag);
    else if (stack.at(-1)?.name === tag.name) {
      const opening = stack.pop()!;
      pairs.set(opening.start, { opening, closing: tag });
    } else stack.length = 0; // Do not reinterpret mismatched/crossing markup.
  }
  let cursor = 0;
  const replacements: Replacement[] = [];
  for (const { opening, closing } of [...pairs.values()].sort((a, b) => a.opening.start - b.opening.start)) {
    if (opening.start < cursor) continue;
    const body = pairedTags(source.slice(opening.end, closing.start), depth + 1);
    replacements.push({ start: opening.start, end: closing.end, value: renderTag(opening, body) });
    cursor = closing.end;
  }
  for (const match of source.matchAll(/\[(br|hr)\]/gi)) {
    if (!isProtected(match.index, ranges) && !isEscaped(source, match.index)) replacements.push({ start: match.index,
      end: match.index + match[0].length, value: match[1].toLowerCase() === 'br' ? '  \n' : '\n\n---\n\n' });
  }
  return replaceRanges(source, replacements);
}

export function normalizeBbCode(source: string): string {
  if (!/\[(?:\/?[a-z]|\*)/i.test(source)) return source;
  return pairedTags(codeBlocks(source), 0);
}
