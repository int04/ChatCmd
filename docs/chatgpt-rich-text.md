# Task chat rich-text rendering

`web/src/tasks/rich-text/ChatRichText.tsx` is the shared display-only renderer used by
`TaskTurnBubble` (user messages, MCP progress and final replies) and
`TurnThinkingSources` (live and saved public browser content). Stored event text is
not rewritten, so existing history is rendered by the same implementation.

## Supported formats

| Input | Presentation |
| --- | --- |
| `:::writing{variant="document" id="58321" title="Title"} … :::` | Document card, title and copy-body action; paragraph and soft line breaks retained. |
| `:::writing{variant="email" subject="Subject" recipient="reader@example.com"} … :::` | Email draft card with subject/recipient; no send action. `title` and `to` aliases are accepted. |
| CommonMark / GFM | Headings, emphasis, quotes, lists, task lists, tables, links, images, strikeout, code and footnotes. |
| `\(...\)`, `\[...\]`, `$...$`, `$$...$$` | Lazy-loaded KaTeX with MathML, bounded expansion and `trust: false`. Common monetary amounts remain text. |
| `[b]`, `[strong]`, `[i]`, `[em]`, `[s]`, `[strike]`, `[del]` | Bold, italic and strikeout, including nested formatting. |
| `[u]`, `[sub]`, `[sup]`, `[kbd]`, `[mark]`, `[h1]` through `[h6]` | Corresponding safe text formatting. |
| `[quote=author]`, `[list]`, `[list=1]`, `[*]`, `[ul]`, `[ol]`, `[li]` | Quotes and lists. |
| `[url=…]`, `[email]`, `[img=alt]` | Sanitized links and HTTP(S) images; external links use opener isolation. |
| `[code=language]`, `[pre]`, fenced Markdown | Literal source with language label and copy action. Source is never executed. |
| `[table]`, `[tr]`, `[th]`, `[td]` | Keyboard-focusable, horizontally scrollable tables. |
| `[details=title]`, `[spoiler=title]` | Native accessible disclosure. |
| `[br]`, `[hr]`, `[left]`, `[right]`, `[center]`, `[justify]` | Line/separator/alignment formatting. |
| `[color=…]`, `[size=1..7]`, `[font=…]` | Fixed CSS classes only: named red/orange/yellow/green/blue/purple/gray, sizes 1–7, monospace/serif/sans-serif. Other values retain text without arbitrary styling. |
| GFM `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]` quotes | Localized visible labels. |
| Safe inline/block HTML | Reparsed then filtered with `rehype-sanitize`; no scripts, event handlers, frames or arbitrary inline styles. |

Writing attributes may be reordered, quoted with either quote style, or unquoted.
Same-line and multiline bodies/closers are supported. A complete header with an
unfinished body renders the available text, without claiming generation is still
active. An incomplete header stays visible until it is complete. Multiple cards
are supported, including repeated external IDs; actual DOM IDs are generated per
component. Header text cannot inject HTML.

CommonMark source positions protect inline, indented, nested and variable-length
fenced code. BBCode code/pre regions are also protected. Malformed or unknown
markup is preserved rather than silently discarded. Markdown footnotes have
per-instance sanitized IDs, and both their links and accessible descriptions stay
within the corresponding message.

## Native annotations and intentional limits

ChatGPT citation/file-citation tokens become readable numbered reference badges.
Their original reference IDs and file line ranges remain available in the title;
no URL is invented from an opaque ID. Entity tokens show their supplied name.

Image carousels, navigation/file lists, products, finance, weather, sports, video,
GenUI and other complete native widget tokens use an explicit fallback explaining
that the original ChatGPT view contains the interactive content. Available titles
are retained. These are **not** functional replicas of native ChatGPT widgets.
`BrowserThought` currently exposes only `id`, `kind` and `content`; it does not carry
structured source URLs, chart series, product inventories or image assets.

Sandbox attachment URLs are not mapped to fabricated local downloads. Unusable
links/images retain a readable label. Mermaid/HTML/JavaScript code blocks remain
source code, not executable previews. The renderer does not provide Canvas editing,
email sending, or arbitrary BBCode dialects. Additional native formats should be
added with observed fixtures, metadata contracts and security tests, not guesses
about opaque references.

## Validation

```sh
cd web
npm test -- --run src/tasks/rich-text
npm test -- --run
npm run lint
npm run build
```

The rich-text suite exercises the exact reported Vietnamese poem, writing/email
metadata, incomplete and repeated blocks, Unicode, code protection, BBCode/GFM,
math, widget fallbacks, copy feedback, XSS filtering, footnote isolation and actual
task-bubble/browser-snapshot integration. DOM tests run in jsdom; they do not prove
pixel layout in a live authenticated browser session.

References consulted:
- OpenAI writing/code blocks: https://help.openai.com/en/articles/20001246-working-with-writing-blocks-and-code-blocks-in-chatgpt
- React Markdown: https://github.com/remarkjs/react-markdown
- Sanitization and math plugin ordering: https://github.com/rehypejs/rehype-sanitize
- KaTeX security: https://katex.org/docs/security
