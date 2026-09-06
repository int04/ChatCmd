import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { ChatRichText } from './ChatRichText';

const poem = `:::writing{variant="document" id="58321" title="Một chút dịu dàng"} Chiều nghiêng qua cửa rất êm, Gió đem nỗi nhớ đặt bên vai người. Ngoài kia phố vẫn đầy vơi, Trong tim chỉ có một lời: thương em.

Trăng lên khâu vá màn đêm, Sao rơi khe khẽ bên thềm cô đơn. Nếu mai giông gió nhiều hơn, Anh xin làm một con đường em qua.

Tháng năm rồi cũng trôi xa, Chỉ mong mình vẫn thật thà cạnh nhau. Chẳng cần hứa chuyện mai sau, Hôm nay thương đủ — đã màu bình yên. :::`;

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('writing UI', () => {
  it('renders the exact reported poem as a titled document with all three paragraphs', () => {
    const { container } = render(<ChatRichText content={poem} />);
    const card = screen.getByRole('region', { name: 'Một chút dịu dàng' });
    expect(card.querySelectorAll('.chat-writing-body p')).toHaveLength(3);
    expect(card).toHaveTextContent('Chiều nghiêng qua cửa rất êm');
    expect(card).toHaveTextContent('Anh xin làm một con đường em qua.');
    expect(card).toHaveTextContent('Hôm nay thương đủ — đã màu bình yên.');
    expect(container.textContent).not.toContain(':::');
    expect(container.textContent).not.toContain('variant=');
  });
  it('preserves source line breaks and renders Markdown inside a document', () => {
    const { container } = render(<ChatRichText content={':::writing{title="Poetry"}\nFirst line\nSecond line\n\n**Bold** and *soft*\n:::'} />);
    expect(container.querySelector('.chat-writing-body p')?.textContent).toBe('First line\nSecond line');
    expect(screen.getByText('Bold').tagName).toBe('STRONG');
    expect(screen.getByText('soft').tagName).toBe('EM');
  });
  it('supports email subject and recipient without turning the draft into a send action', () => {
    render(<ChatRichText content={':::writing{variant="email" subject="Project update" recipient="team@example.com"} Hello **team**. :::'} />);
    expect(screen.getByRole('region', { name: 'Project update' })).toHaveTextContent('team@example.com');
    expect(screen.getByText('team').tagName).toBe('STRONG');
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });
  it('updates a streaming document rather than duplicating it', () => {
    const { container, rerender } = render(<ChatRichText content={':::writing{title="Draft"} First'} />);
    expect(screen.getByRole('region', { name: 'Draft' })).toHaveTextContent('First');
    rerender(<ChatRichText content={':::writing{title="Draft"} First and last :::'} />);
    expect(container.querySelectorAll('.chat-writing-card')).toHaveLength(1);
    expect(screen.getByRole('region', { name: 'Draft' })).toHaveTextContent('First and last');
    expect(container.textContent).not.toContain(':::');
  });
  it('copies only the document body without protocol metadata', async () => {
    render(<ChatRichText content={':::writing{title="Copy me"} **Body**\n\nLast line :::'} />);
    fireEvent.click(screen.getByRole('button', { name: /Copy|Sao chép/ }));
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith('**Body**\n\nLast line'));
  });
  it('reports clipboard failure accessibly', async () => {
    vi.mocked(navigator.clipboard.writeText).mockRejectedValueOnce(new Error('Denied'));
    render(<ChatRichText content={':::writing{} Body :::'} />);
    fireEvent.click(screen.getByRole('button', { name: /Copy|Sao chép/ }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(/Could not copy|Không sao chép/));
  });
  it('treats writing metadata as text, never executable HTML or DOM IDs', () => {
    const { container } = render(<ChatRichText content={':::writing{id="location" title="<img src=x onerror=alert(1)>"} Safe :::'} />);
    expect(screen.getByRole('heading', { name: '<img src=x onerror=alert(1)>' })).toBeInTheDocument();
    expect(container.querySelector('img, #location, [onerror]')).toBeNull();
  });
});

describe('Markdown, HTML and legacy BBCode', () => {
  it.each([['b', 'STRONG'], ['strong', 'STRONG'], ['i', 'EM'], ['em', 'EM'], ['s', 'DEL'], ['strike', 'DEL'],
    ['u', 'U'], ['sub', 'SUB'], ['sup', 'SUP'], ['mark', 'MARK'], ['kbd', 'KBD']])('renders [%s] safely', (tag, expected) => {
    render(<ChatRichText content={`[${tag}]Formatted[/${tag}]`} />);
    expect(screen.getByText('Formatted').tagName).toBe(expected);
  });
  it('supports nested formatting, quotes, ordered/unordered lists and headings', () => {
    const { container } = render(<ChatRichText content={'[h2]Heading[/h2]\n\n[b]Bold [i]nested[/i][/b]\n\n[quote=Alice]Quote[/quote]\n\n[list][*]One[*]Two[/list]\n\n[list=1][*]Three[*]Four[/list]'} />);
    expect(screen.getByRole('heading', { name: 'Heading', level: 2 })).toBeInTheDocument();
    expect(screen.getByText('nested').closest('strong')).not.toBeNull();
    expect(container.querySelector('blockquote')).toHaveTextContent('Alice');
    expect(container.querySelectorAll('li')).toHaveLength(4);
    expect(container.querySelectorAll('ol')).toHaveLength(1);
    expect(container.querySelectorAll('ul')).toHaveLength(1);
  });
  it('renders GFM tables, tasks, strikeout, autolinks and alerts', () => {
    const { container } = render(<ChatRichText content={'| Name | Value |\n| --- | --- |\n| A | B |\n\n- [x] Done\n- [ ] Pending\n\n~~Removed~~\n\nhttps://example.com\n\n> [!NOTE]\n> Remember'} />);
    expect(screen.getByRole('table')).toHaveTextContent('NameValueAB');
    expect(screen.getAllByRole('checkbox')).toHaveLength(2);
    expect(screen.getAllByRole('checkbox')[0]).toBeChecked();
    expect(screen.getByText('Removed').tagName).toBe('DEL');
    expect(screen.getByRole('link')).toHaveAttribute('rel', 'noreferrer noopener');
    expect(container.querySelector('blockquote')).toHaveTextContent('Remember');
    expect(container.textContent).not.toContain('[!NOTE]');
  });
  it('renders BBCode tables, safe inline HTML and collapsible spoilers', () => {
    const { container } = render(<ChatRichText content={'[table][tr][th]Name[/th][/tr][tr][td]Value[/td][/tr][/table]\n\n[spoiler=Details]Hidden **bold**[/spoiler]\n\nH<sub>2</sub>O <sup>2</sup><br>Next'} />);
    expect(screen.getByRole('table')).toHaveTextContent('NameValue');
    expect(container.querySelector('summary')).toHaveTextContent('Details');
    expect(container.querySelector('details strong')).toHaveTextContent('bold');
    expect(container.querySelector('sub')).toHaveTextContent('2');
    expect(container.querySelector('sup')).toHaveTextContent('2');
    expect(container.querySelector('br')).not.toBeNull();
  });
  it('limits styling to the allowlist instead of accepting arbitrary CSS', () => {
    const { container } = render(<ChatRichText content={'[color=red]Red[/color] [size=5]Large[/size] [font=monospace]Mono[/font]\n\n[color=expression(alert(1))]Still visible[/color]'} />);
    expect(screen.getByText('Red')).toHaveClass('chat-color-red');
    expect(screen.getByText('Large')).toHaveClass('chat-size-5');
    expect(screen.getByText('Mono')).toHaveClass('chat-font-monospace');
    expect(container.querySelector('[style]')).toBeNull();
    expect(container).toHaveTextContent('Still visible');
  });
  it('renders safe BBCode links and images with defensive browser attributes', () => {
    render(<ChatRichText content={'[url=https://example.com/a]Example[/url] [email]a@example.com[/email]\n\n[img=Example image]https://example.com/image.png[/img]'} />);
    expect(screen.getByRole('link', { name: 'Example' })).toHaveAttribute('href', 'https://example.com/a');
    expect(screen.getByRole('link', { name: 'a@example.com' })).toHaveAttribute('href', 'mailto:a@example.com');
    expect(screen.getByRole('img')).toHaveAttribute('loading', 'lazy');
    expect(screen.getByRole('img')).toHaveAttribute('referrerpolicy', 'no-referrer');
  });
  it.each(['javascript:alert(1)', 'data:text/html,<script>alert(1)</script>', 'vbscript:msgbox(1)'])('rejects unsafe URL %s in HTML and BBCode', (url) => {
    const { container } = render(<ChatRichText content={`[url="${url}"]Unsafe[/url]\n\n<img src="${url}" onerror="alert(1)"><a href="${url}" onclick="alert(1)">Raw</a>`} />);
    expect(container.querySelector('script, iframe, [onclick], [onerror], a[href], img[src]')).toBeNull();
    expect(container).toHaveTextContent('Unsafe');
  });
  it('sanitizes script/style/event injection without breaking normal Markdown', () => {
    const { container } = render(<ChatRichText content={'<script>alert(1)</script>\n\n<iframe src="https://example.com"></iframe>\n\n<span style="position:fixed" onclick="alert(1)">Text</span>\n\n**Safe**'} />);
    expect(container.querySelector('script, iframe, [onclick], [style]')).toBeNull();
    expect(screen.getByText('Safe').tagName).toBe('STRONG');
  });
  it('keeps unknown, incomplete and escaped markup readable', () => {
    const { container } = render(<ChatRichText content={'[unknown]Content[/unknown]\n\n[b]Incomplete\n\n\\[b]Literal\\[/b]'} />);
    expect(container).toHaveTextContent('[unknown]Content[/unknown]');
    expect(container).toHaveTextContent('[b]Incomplete');
    expect(container).toHaveTextContent('[b]Literal[/b]');
  });
  it('protects code samples from all rich-text transformations and copies original code', async () => {
    const code = ':::writing{title="literal"} body :::\n[b]keep[/b]\n\\(x\\)\nciteturn0search0';
    const { container } = render(<ChatRichText content={`\`\`\`text\n${code}\n\`\`\``} />);
    expect(container.querySelector('.chat-code-block code')?.textContent).toBe(code + '\n');
    expect(container.querySelector('.chat-writing-card, .chat-reference, .katex')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Copy|Sao chép/ }));
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith(code));
  });
  it('supports BBCode code with embedded backticks without activating its content', () => {
    const { container } = render(<ChatRichText content={'[code=js]```\n[b]literal[/b]\n:::writing{} sample :::[/code]'} />);
    expect(container.querySelector('pre code')?.textContent).toContain('[b]literal[/b]');
    expect(container.querySelector('pre code')?.textContent).toContain(':::writing{} sample :::');
    expect(container.querySelector('.chat-writing-card')).toBeNull();
  });
  it('keeps footnote links connected and IDs unique across messages', () => {
    const { container } = render(<><ChatRichText content={'First[^1]\n\n[^1]: Footnote'} /><ChatRichText content={'Second[^1]\n\n[^1]: Another footnote'} /></>);
    const ids = [...container.querySelectorAll('[id]')].map((element) => element.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const link of container.querySelectorAll<HTMLAnchorElement>('a[href^="#"]')) {
      const target = document.getElementById(decodeURIComponent(link.hash.slice(1)));
      expect(target).not.toBeNull();
      expect(target?.closest('.chat-rich-text')).toBe(link.closest('.chat-rich-text'));
      for (const id of (link.getAttribute('aria-describedby') || '').split(/\s+/).filter(Boolean)) {
        expect(document.getElementById(id)?.closest('.chat-rich-text')).toBe(link.closest('.chat-rich-text'));
      }
    }
  });
});

describe('native ChatGPT annotations', () => {
  it('renders entities and readable citation fallbacks without inventing URLs', () => {
    const { container } = render(<ChatRichText content={'entity["city","Hà Nội","Thủ đô"] citeturn0search0turn1view0 citeturn0search0 fileciteturn2file0L10-L20'} />);
    expect(screen.getByText('Hà Nội')).toHaveClass('chat-entity');
    const citations = container.querySelectorAll('.chat-reference');
    expect(citations).toHaveLength(3);
    expect(citations[0]).toHaveTextContent('1, 2');
    expect(citations[1]).toHaveTextContent('1');
    expect(citations[2]).toHaveAttribute('title', expect.stringContaining('L10-L20'));
    expect(container.querySelector('a')).toBeNull();
    expect(container.textContent).not.toContain('');
  });
  it.each(['i', 'image_group', 'navlist', 'filenavlist', 'products', 'finance', 'forecast', 'schedule', 'standing', 'video', 'genui', 'future_widget'])('gives %s an explicit no-metadata fallback', (type) => {
    const { container } = render(<ChatRichText content={`${type}turn0reference0`} />);
    expect(container.querySelector('.chat-widget')).toHaveTextContent(/ChatGPT/);
    expect(container.querySelector('.chat-widget')).toHaveAttribute('title', expect.stringContaining('turn0reference0'));
    expect(container.querySelector('a, img, iframe')).toBeNull();
    expect(container.textContent).not.toContain('');
  });
  it('retains available widget titles and escapes hostile entity labels', () => {
    const { container } = render(<ChatRichText content={'navlistUseful articlesturn0news0\n\nproducts{"selections":[["ref","Desk lamp"]]}\n\nentity["x","<img src=x onerror=alert(1)>","safe"]'} />);
    expect(container).toHaveTextContent('Useful articles');
    expect(container).toHaveTextContent('Desk lamp');
    expect(container).toHaveTextContent('<img src=x onerror=alert(1)>');
    expect(container.querySelector('img, [onerror]')).toBeNull();
  });
});

describe('math', () => {
  it('renders dollar, parenthesis and bracket math with accessible MathML', async () => {
    const { container } = render(<ChatRichText content={'Inline $x^2$ and \\(\\frac{1}{2}\\).\n\n\\[\\sum_{n=1}^{3} n\\]\n\n$$\nx + y = z\n$$'} />);
    await waitFor(() => expect(container.querySelectorAll('.katex')).toHaveLength(4));
    expect(container.querySelectorAll('math')).toHaveLength(4);
    expect(container.querySelectorAll('.katex-display')).toHaveLength(2);
  });
  it('does not interpret prices as mathematics', () => {
    const { container } = render(<ChatRichText content={'Prices are $20 and $30. Save $5 today.'} />);
    expect(container).toHaveTextContent('Prices are $20 and $30. Save $5 today.');
    expect(container.querySelector('.katex')).toBeNull();
  });
  it('never enables trusted KaTeX commands that inject links or HTML', async () => {
    const { container } = render(<ChatRichText content={'$\\href{javascript:alert(1)}{Click}$ $\\htmlClass{evil}{X}$'} />);
    await waitFor(() => expect(container.querySelectorAll('.katex')).toHaveLength(2));
    expect(container.querySelector('a[href^="javascript:"], .evil, script, iframe')).toBeNull();
  });
});
