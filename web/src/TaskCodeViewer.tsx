import { Highlight, themes, type Language } from 'prism-react-renderer';

export type TaskCodeViewerProps = {
  code: string;
  path?: string | null;
  language?: string;
  startLine?: number;
  label?: string;
  highlightedLines?: Record<number, 'added' | 'removed' | 'changed'>;
};

const LANGUAGES: Record<string, Language> = {
  bat: 'bash', c: 'c', cmd: 'bash', cpp: 'cpp', cs: 'csharp', css: 'css', diff: 'diff', go: 'go',
  html: 'markup', java: 'java', js: 'javascript', json: 'json', jsx: 'jsx', md: 'markdown', ps1: 'powershell',
  py: 'python', rs: 'rust', scss: 'scss', sh: 'bash', sql: 'sql', ts: 'typescript', tsx: 'tsx', xml: 'markup',
  yaml: 'yaml', yml: 'yaml',
};

export function languageFromPath(path?: string | null): Language {
  if (!path) return 'plain';
  const file = path.split(/[\\/]/).at(-1)?.toLowerCase() ?? '';
  if (file === 'dockerfile') return 'docker';
  const ext = file.includes('.') ? file.split('.').at(-1) ?? '' : '';
  return LANGUAGES[ext] ?? 'plain';
}

export function TaskCodeViewer({ code, path, language, startLine = 1, label, highlightedLines }: TaskCodeViewerProps) {
  const resolved = (language as Language | undefined) ?? languageFromPath(path);
  const normalized = code.replace(/\r\n?/g, '\n').replace(/\n$/, '') || ' ';
  const firstLine = Math.max(1, Number.isFinite(startLine) ? Math.trunc(startLine) : 1);
  return <div className="task-code-viewer">
    <div className="task-code-titlebar" aria-hidden="true">
      <span className="task-code-dots"><i /><i /><i /></span>
      <span>{path?.split(/[\\/]/).at(-1) || label || resolved}</span>
      <small>{resolved === 'plain' ? 'TEXT' : resolved.toUpperCase()}</small>
    </div>
    <Highlight theme={themes.vsDark} code={normalized} language={resolved}>
      {({ className, style, tokens, getLineProps, getTokenProps }) => <pre className={`${className} task-code-scroll`} style={style} tabIndex={0} aria-label={label}>
        <code>{tokens.map((line, index) => { const lineNumber = firstLine + index; const mark = highlightedLines?.[lineNumber]; return <span {...getLineProps({ line })} className={`task-code-line ${mark ? `diff-${mark}` : ''}`} key={index}>
          <span className="task-code-line-number" aria-hidden="true">{lineNumber}</span>
          <span className="task-code-line-content">{line.map((token, tokenIndex) => <span {...getTokenProps({ token })} key={tokenIndex} />)}</span>
        </span>; })}</code>
      </pre>}
    </Highlight>
  </div>;
}
