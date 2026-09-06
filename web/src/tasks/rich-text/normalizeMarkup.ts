import { normalizeBbCode } from './bbcode';
import { richTextLabels, type RichTextLabels } from './labels';
import { escapeHtml, isEscaped, isProtected, literalText, protectedRanges, replaceRanges, type Replacement } from './sourceRanges';

function tokenLabel(type: string, labels: RichTextLabels): string {
  const keys: Record<string, keyof RichTextLabels> = {
    i: 'images', image: 'images', image_group: 'images', navlist: 'links', filenavlist: 'files',
    products: 'products', product: 'products', finance: 'finance', forecast: 'weather', weather: 'weather',
    schedule: 'sports', standing: 'sports', standings: 'sports', sports: 'sports', video: 'video', audio: 'audio',
    map: 'map', table: 'table', chart: 'chart',
  };
  return keys[type] ? labels[keys[type]] : `${labels.widget} (${type})`;
}

function readablePayload(type: string, fields: string[]): string {
  if (type === 'navlist') return fields[0] ?? '';
  if (type === 'filenavlist') return fields.filter((field) => field.includes('\uE205')).map((field) => field.split('\uE205').slice(1).join(' ')).join(' · ');
  if (type === 'products' || type === 'genui') {
    try {
      const value: unknown = JSON.parse(fields.join('\uE202'));
      if (!value || typeof value !== 'object') return '';
      if ('selections' in value && Array.isArray(value.selections)) return value.selections
        .flatMap((selection: unknown) => Array.isArray(selection) && typeof selection[1] === 'string' ? [selection[1]] : []).join(' · ');
      if ('email_preview' in value && value.email_preview && typeof value.email_preview === 'object'
        && 'subject' in value.email_preview && typeof value.email_preview.subject === 'string') return value.email_preview.subject;
    } catch { /* Malformed widget data is still retained in the fallback title. */ }
  }
  return '';
}

function normalizeTokens(source: string, labels: RichTextLabels, references: Map<string, number>): string {
  if (!source.includes('\uE200')) return source;
  const ranges = protectedRanges(source);
  const replacements: Replacement[] = [];
  for (const match of source.matchAll(/\uE200([^\uE200\uE201]*)\uE201/g)) {
    if (isProtected(match.index, ranges) || isEscaped(source, match.index)) continue;
    const [type, ...fields] = match[1].split('\uE202');
    if (!/^[a-z][a-z0-9_]*$/i.test(type)) continue;
    let value: string;
    if (type === 'entity') {
      try {
        const entity: unknown = JSON.parse(fields.join('\uE202'));
        if (!Array.isArray(entity) || typeof entity[1] !== 'string') continue;
        value = `<span class="chat-entity" title="${escapeHtml(typeof entity[2] === 'string' ? entity[2] : '')}">${literalText(entity[1])}</span>`;
      } catch { continue; }
    } else if (type === 'cite' || type === 'filecite') {
      const ids = fields.filter(Boolean);
      const numbers = ids.filter((id) => !/^L\d+(?:-L?\d+)?$/.test(id)).map((id) => {
        if (!references.has(id)) references.set(id, references.size + 1);
        return references.get(id);
      });
      const label = `${type === 'filecite' ? labels.fileSource : labels.source} ${numbers.join(', ')}`;
      value = `<span class="chat-reference" title="${escapeHtml(`${labels.missingSource}\n${ids.join(' · ')}`)}">[${literalText(label)}]</span>`;
    } else {
      const detail = readablePayload(type, fields);
      const label = tokenLabel(type, labels) + (detail ? `: ${detail}` : '');
      // Native widgets require metadata, not fabricated source URLs or external fetches.
      value = `<span class="chat-widget" title="${escapeHtml(match[0])}"><strong>${literalText(label)}</strong> — ${literalText(labels.missingWidget)}</span>`;
    }
    replacements.push({ start: match.index, end: match.index + match[0].length, value });
  }
  return replaceRanges(source, replacements);
}

function normalizeMathAndAlerts(source: string, labels: RichTextLabels): string {
  if (!/[\\$]|\[!/.test(source)) return source;
  const ranges = protectedRanges(source);
  const replacements: Replacement[] = [];
  const add = (match: RegExpMatchArray, value: string) => {
    const start = match.index!;
    if (!isProtected(start, ranges) && !isEscaped(source, start)) replacements.push({ start, end: start + match[0].length, value });
  };
  for (const match of source.matchAll(/\\\[([\s\S]*?)\\\]/g)) add(match, `\n\n$$\n${match[1].trim()}\n$$\n\n`);
  for (const match of source.matchAll(/\\\(([^\n]*?)\\\)/g)) add(match, `$$${match[1]}$$`);
  // Avoid turning common monetary amounts ('$20 and $30') into inline math.
  for (const match of source.matchAll(/\$\d[\d,]*(?:\.\d{1,2})?(?=[\s.,!?;:]|$)/g)) {
    const start = match.index;
    if (source[start - 1] === '$') continue;
    const closing = source.indexOf('$', start + match[0].length);
    const inside = closing < 0 ? '' : source.slice(start + 1, closing);
    if (inside && !inside.includes('\n') && /[\\^_=+*/{}]/.test(inside)) continue;
    add(match, '\\' + match[0]);
  }
  for (const match of source.matchAll(/^(\s*>[ \t]*)\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]/gm)) {
    const key = match[2].toLowerCase() as 'note' | 'tip' | 'important' | 'warning' | 'caution';
    add(match, `${match[1]}**${labels[key]}**`);
  }
  return replaceRanges(source, replacements);
}

export function normalizeChatMarkup(source: string, vi: boolean, references = new Map<string, number>()): string {
  const labels = richTextLabels(vi);
  return normalizeMathAndAlerts(normalizeTokens(normalizeBbCode(source), labels, references), labels);
}
