import { memo, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import oneDark from 'react-syntax-highlighter/dist/esm/styles/prism/one-dark';
import DiffView from './DiffView';

interface Props {
  content: string;
  /** Smaller typography for reasoning/activity rows. */
  compact?: boolean;
  className?: string;
}

/** Code blocks larger than this skip syntax highlighting (perf guard). */
const MAX_HIGHLIGHT_CHARS = 40000;

function CodeCopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      onClick={() => {
        navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1800);
        });
      }}
      className="absolute top-2 right-2 p-1.5 rounded-md bg-zinc-800/80 text-zinc-400 hover:text-zinc-200 hover:bg-zinc-700 transition-colors"
      title="Copy code"
      aria-label="Copy code"
    >
      {copied ? <Check size={13} className="text-emerald-400" /> : <Copy size={13} />}
    </button>
  );
}

function looksLikeDiff(lang: string, text: string): boolean {
  if (lang === 'diff' || lang === 'patch') return true;
  return text.startsWith('diff --git') || text.startsWith('--- ') || text.includes('\n@@ ');
}

function CodeBlock({ lang, text }: { lang: string; text: string }) {
  if (looksLikeDiff(lang, text)) {
    return (
      <div className="my-5 overflow-hidden rounded-xl border border-zinc-800/60">
        <DiffView diff={text} />
      </div>
    );
  }
  const highlight = !!lang && text.length <= MAX_HIGHLIGHT_CHARS;
  return (
    <div className="relative group my-5">
      <div className="flex items-center justify-between rounded-t-xl border border-zinc-800/60 border-b-0 bg-zinc-900/70 px-3 py-1">
        <span className="text-[10px] font-mono text-zinc-500 uppercase tracking-wider">{lang || 'text'}</span>
      </div>
      <div className="border border-zinc-800/60 rounded-b-xl overflow-x-auto text-[13.5px] leading-relaxed">
        {highlight ? (
          <SyntaxHighlighter
            language={lang}
            style={oneDark}
            customStyle={{
              margin: 0,
              padding: '16px',
              background: '#09090b',
              fontSize: '13.5px',
              lineHeight: '1.6',
            }}
            codeTagProps={{ style: { fontFamily: "'JetBrains Mono', ui-monospace, monospace" } }}
          >
            {text}
          </SyntaxHighlighter>
        ) : (
          <pre className="p-4 bg-zinc-950 text-zinc-300 font-mono text-[13.5px] whitespace-pre">
            <code>{text}</code>
          </pre>
        )}
      </div>
      <CodeCopyButton text={text} />
    </div>
  );
}

function Markdown({ content, compact = false, className = '' }: Props) {
  if (!content) return null;

  return (
    <div className={`markdown-body select-text leading-[1.85] ${compact ? 'markdown-compact' : ''} ${className}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          pre: ({ children }) => <>{children}</>,
          code: ({ className: codeClass, children }) => {
            const match = /language-([\w+-]+)/.exec(codeClass || '');
            const text = String(children ?? '').replace(/\n$/, '');
            const block = !!match || text.includes('\n');
            if (!block) {
              return (
                <code className="bg-zinc-800/80 px-1.5 py-0.5 rounded text-[13.5px] font-mono text-indigo-300 border border-zinc-700/50">
                  {children}
                </code>
              );
            }
            return <CodeBlock lang={match?.[1] ?? ''} text={text} />;
          },
          a: ({ href, children }) => (
            <a
              href={href}
              target="_blank"
              rel="noopener noreferrer"
              className="text-blue-400 hover:text-blue-300 underline underline-offset-2 decoration-blue-500/40 hover:decoration-blue-400 transition-colors"
            >
              {children}
            </a>
          ),
          img: ({ src, alt }) => (
            <img
              src={src}
              alt={alt || ''}
              loading="lazy"
              className="md-img-fade max-w-full rounded-lg my-2 border border-zinc-700"
              style={{ maxHeight: '320px' }}
              onLoad={(e) => e.currentTarget.classList.add('is-loaded')}
            />
          ),
          table: ({ children }) => (
            <div className="overflow-x-auto my-5 rounded-xl border border-white/[0.12] bg-zinc-950/20">
              <table className="w-full text-left border-collapse text-[14.5px] leading-relaxed">{children}</table>
            </div>
          ),
          thead: ({ children }) => <thead className="bg-zinc-900/40">{children}</thead>,
          th: ({ children }) => (
            <th className="px-5 py-3.5 border-b border-white/[0.15] font-semibold text-zinc-300 text-[13px] uppercase tracking-wider">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="px-5 py-3.5 text-zinc-300 border-b border-white/[0.08] align-top">{children}</td>
          ),
          tr: ({ children }) => <tr className="hover:bg-white/[0.02] transition-colors">{children}</tr>,
          blockquote: ({ children }) => (
            <blockquote className="border-l-2 border-indigo-500/50 pl-4 my-4 text-zinc-300 italic bg-indigo-500/5 rounded-r-lg py-2">
              {children}
            </blockquote>
          ),
          hr: () => <hr className="border-zinc-800 my-6" />,
          h1: ({ children }) => <h1 className="text-[21px] font-semibold text-zinc-100 mt-6 mb-3">{children}</h1>,
          h2: ({ children }) => <h2 className="text-[17.5px] font-semibold text-zinc-200 mt-5 mb-2.5 border-b border-zinc-800/40 pb-1.5">{children}</h2>,
          h3: ({ children }) => <h3 className="text-[15px] font-medium text-zinc-300 mt-4 mb-2">{children}</h3>,
          h4: ({ children }) => <h4 className="text-[14.5px] font-medium text-zinc-300 mt-3 mb-1.5">{children}</h4>,
          p: ({ children }) => <p className="text-zinc-200 my-3 break-words">{children}</p>,
          ul: ({ children }) => <ul className="list-disc ml-6 my-2.5 space-y-1">{children}</ul>,
          ol: ({ children }) => <ol className="list-decimal ml-7 my-2.5 space-y-1.5">{children}</ol>,
          li: ({ children }) => <li className="text-zinc-200 pl-0.5">{children}</li>,
          strong: ({ children }) => <strong className="font-semibold text-zinc-50">{children}</strong>,
          em: ({ children }) => <em className="italic text-zinc-200">{children}</em>,
          del: ({ children }) => <del className="text-zinc-500 line-through">{children}</del>,
          input: (props) => (
            <input
              {...props}
              disabled
              className="mr-1.5 align-middle accent-indigo-500"
            />
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

export default memo(Markdown);
