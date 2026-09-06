import { useId, useMemo, type ComponentProps } from 'react';
import { scopeReferences } from './scopeReferences';
import ReactMarkdown, { defaultUrlTransform, type Components, type ExtraProps, type Options } from 'react-markdown';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize, { defaultSchema, type Options as SanitizeOptions } from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { CopyTextButton } from './CopyTextButton';
import { richTextLabels, type RichTextLabels } from './labels';

const schema: SanitizeOptions = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames ?? []), 'u', 'mark'],
  attributes: {
    ...defaultSchema.attributes,
    code: [['className', /^language-[\w+-]+$/, 'math-inline', 'math-display']],
    span: [...(defaultSchema.attributes?.span ?? []), ['className',
      /^chat-(?:reference|widget|entity|color-(?:red|orange|yellow|green|blue|purple|gray)|size-[1-7]|font-(?:monospace|serif|sans-serif))$/]],
    div: [...(defaultSchema.attributes?.div ?? []), ['align', 'left', 'right', 'center', 'justify']],
  },
};

type TextNode = { value?: string; children?: TextNode[] };
function textContent(node: TextNode): string { return node.value ?? node.children?.map(textContent).join('') ?? ''; }

function CodeBlock({ node, children, labels, ...props }: ComponentProps<'pre'> & ExtraProps & { labels: RichTextLabels }) {
  const codeNode = node?.children.find((child) => child.type === 'element' && child.tagName === 'code');
  const classes = codeNode?.type === 'element' ? codeNode.properties.className : [];
  const language = (Array.isArray(classes) ? classes : []).map(String).find((name) => name.startsWith('language-'))?.slice(9);
  const text = node ? textContent(node).replace(/\n$/, '') : '';
  return <div className="chat-code-block">
    <div className="chat-code-toolbar"><span>{language || labels.code}</span><CopyTextButton text={text} labels={labels} /></div>
    <pre {...props} tabIndex={0}>{children}</pre>
  </div>;
}

export function MarkdownContent({ content, vi, extraRemark = [], extraRehype = [] }: {
  content: string; vi: boolean; extraRemark?: NonNullable<Options['remarkPlugins']>; extraRehype?: NonNullable<Options['rehypePlugins']>;
}) {
  const components = useMemo<Components>(() => {
    const labels = richTextLabels(vi);
    return {
      a: ({ node: _node, href, children, ...props }) => href
        ? <a {...props} href={href} target={href.startsWith('#') ? undefined : '_blank'} rel="noreferrer noopener">{children}</a>
        : <span className="chat-unavailable-link" title={labels.unavailableLink}>{children}</span>,
      img: ({ node: _node, src, alt, ...props }) => typeof src === 'string' && /^https?:\/\//i.test(src)
        ? <img {...props} src={src} alt={alt || ''} loading="lazy" decoding="async" referrerPolicy="no-referrer" />
        : <span className="chat-unavailable-image">{alt || labels.unavailableImage}</span>,
      pre: (props) => <CodeBlock {...props} labels={labels} />,
      table: ({ node: _node, children, ...props }) => <div className="chat-table-scroll" role="region" aria-label={labels.table} tabIndex={0}><table {...props}>{children}</table></div>,
    };
  }, [vi]);
  const instanceId = useId();
  const prefix = `chat-rich-${instanceId.replace(/[^\w-]/g, '')}-`;
  const scopedSchema = useMemo(() => ({ ...schema, clobberPrefix: prefix }), [prefix]);
  return <ReactMarkdown remarkRehypeOptions={{ clobberPrefix: '' }} remarkPlugins={[remarkGfm, ...extraRemark]} rehypePlugins={[
    rehypeRaw, [rehypeSanitize, scopedSchema], [scopeReferences, { prefix }], ...extraRehype,
  ]} components={components} urlTransform={defaultUrlTransform}>{content}</ReactMarkdown>;
}
