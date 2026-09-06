import { describe, expect, it } from 'vitest';
import { splitWritingBlocks } from './writingBlocks';

const envelope = ':::writing{variant="document" id="58321" title="Một chút dịu dàng"}';
const body = 'Chiều nghiêng qua cửa rất êm, Gió đem nỗi nhớ đặt bên vai người.\n\nTháng năm rồi cũng trôi xa.';

describe('ChatGPT writing envelopes', () => {
  it('accepts the reported same-line header and closing fence', () => {
    expect(splitWritingBlocks(`${envelope} ${body} :::`)).toEqual([
      { kind: 'writing', start: 0, attributes: { variant: 'document', id: '58321', title: 'Một chút dịu dàng' }, content: body, closed: true },
    ]);
  });
  it('preserves text surrounding multiple blocks and repeated document IDs', () => {
    const parts = splitWritingBlocks(`Before\n\n${envelope}\nOne\n:::\n\nBetween\n\n${envelope} Two :::\nAfter`);
    expect(parts.map((part) => part.kind)).toEqual(['markdown', 'writing', 'markdown', 'writing', 'markdown']);
    expect(parts.map((part) => part.content.trim())).toEqual(['Before', 'One', 'Between', 'Two', 'After']);
  });
  it('supports reordered, single-quoted, escaped and unquoted attributes', () => {
    const [part] = splitWritingBlocks(':::writing{subject="A \\"quote\\" }" id=123 variant=email recipient=reader@example.com} Draft :::');
    expect(part.kind).toBe('writing');
    if (part.kind === 'writing') expect(part.attributes).toMatchObject({ subject: 'A "quote" }', id: '123', variant: 'email', recipient: 'reader@example.com' });
    expect(splitWritingBlocks(":::writing{title='Tên có dấu'} Hi :::")[0].kind).toBe('writing');
  });
  it('shows a complete header with an unfinished body without losing the text', () => {
    const parts = splitWritingBlocks(`${envelope}\nFirst line\nSecond line`);
    expect(parts[0]).toMatchObject({ kind: 'writing', content: 'First line\nSecond line', closed: false });
    expect(splitWritingBlocks(`${envelope}\nFirst line\nSecond line\n:::`)[0]).toMatchObject({ closed: true });
  });
  it('does not swallow a later block when an earlier block has no closing fence', () => {
    const parts = splitWritingBlocks(`${envelope}\nFirst\n${envelope}\nSecond\n:::`);
    expect(parts).toHaveLength(2);
    expect(parts[0]).toMatchObject({ content: 'First', closed: false });
    expect(parts[1]).toMatchObject({ content: 'Second', closed: true });
  });
  it('ignores a closing marker inside a fenced code sample in a document', () => {
    const content = 'Text\n\n```md\n:::\n```\n\nAfter code';
    expect(splitWritingBlocks(`${envelope}\n${content}\n:::`)[0]).toMatchObject({ content, closed: true });
  });
  it.each([
    ['inline', `\`${envelope} body :::\``],
    ['double backticks', `\`\`${envelope} \`code\` :::\`\``],
    ['fenced', `\`\`\`md\n${envelope}\nbody\n:::\n\`\`\``],
    ['longer fence', `\`\`\`\`md\n\`\`\`\n${envelope}\nbody\n:::\n\`\`\`\``],
    ['tilde fence', `~~~md\n${envelope}\nbody\n:::\n~~~`],
    ['unclosed fence', `\`\`\`md\n${envelope}\nbody\n:::`],
    ['indented', `    ${envelope}\n    body\n    :::`],
    ['nested code', `> \`\`\`md\n> ${envelope}\n> body\n> :::\n> \`\`\``],
    ['BBCode sample', `[code=md]${envelope}\nbody\n:::[/code]`],
    ['HTML sample', `<pre>${envelope}\nbody\n:::</pre>`],
  ])('keeps %s literal', (_label, source) => {
    expect(splitWritingBlocks(source)).toEqual([{ kind: 'markdown', start: 0, content: source }]);
  });
  it.each([':::writing{title="unfinished', ':::writing{broken attribute} text :::', ':::note{} text :::', 'prefix:::writing{} text :::', '\\:::writing{} text :::'])('preserves malformed, unrelated or escaped content: %s', (source) => {
    expect(splitWritingBlocks(source)).toEqual([{ kind: 'markdown', start: 0, content: source }]);
  });
  it('does not accept an opener inside a quoted header value as a second block', () => {
    const source = ':::writing{title="A :::writing{ id=123 } title"} Text :::';
    const parts = splitWritingBlocks(source);
    expect(parts).toHaveLength(1);
    expect(parts[0]).toMatchObject({ content: 'Text', closed: true });
  });
  it('preserves Unicode and all paragraph text for long documents', () => {
    const content = Array.from({ length: 1000 }, (_, index) => `Dòng ${index}: tiếng Việt, 日本語, العربية.`).join('\n\n');
    expect(splitWritingBlocks(`${envelope}\n${content}\n:::`)[0].content).toBe(content);
  });
});
