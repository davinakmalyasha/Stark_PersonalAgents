import React, { useState } from 'react';
import { Copy, Check, Circle, CheckCircle2 } from 'lucide-react';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import oneDark from 'react-syntax-highlighter/dist/esm/styles/prism/one-dark';

interface Props {
  content: string;
}

/** Code blocks larger than this skip syntax highlighting (perf guard). */
const MAX_HIGHLIGHT_CHARS = 40000;

/** Copy button for code blocks */
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
    >
      {copied ? <Check size={13} className="text-emerald-400" /> : <Copy size={13} />}
    </button>
  );
}

export default function Markdown({ content }: Props) {
  if (!content) return null;

  const lines = content.split('\n');
  const blocks: React.ReactNode[] = [];
  let currentTable: string[][] = [];
  let inCodeBlock = false;
  let codeBlockLines: string[] = [];
  let codeBlockLang = '';

  /** Inline renderer: links, images, bold, inline code, strikethrough */
  const renderInline = (text: string) => {
    const parts: React.ReactNode[] = [];
    // We walk the string with a unified regex that matches links, images, bold,
    // inline code, and strikethrough. Order: images first (most specific), then
    // links, then bold/code/strikethrough.
    const regex = /(!\[([^\]]*)\]\(([^)]+)\))|(\[([^\]]+)\]\(([^)]+)\))|(\*\*|`|~~)(.*?)\7/g;
    let lastIdx = 0;
    let match;
    while ((match = regex.exec(text)) !== null) {
      if (match.index > lastIdx) {
        parts.push(text.slice(lastIdx, match.index));
      }
      if (match[1]) {
        // Image: ![alt](url)
        parts.push(
          <img
            key={match.index}
            src={match[3]}
            alt={match[2]}
            loading="lazy"
            className="md-img-fade max-w-full rounded-lg my-2 border border-zinc-700"
            style={{ maxHeight: '320px' }}
            onLoad={(e) => e.currentTarget.classList.add('is-loaded')}
          />
        );
      } else if (match[4]) {
        // Link: [text](url)
        parts.push(
          <a
            key={match.index}
            href={match[6]}
            target="_blank"
            rel="noopener noreferrer"
            className="text-blue-400 hover:text-blue-300 underline underline-offset-2 decoration-blue-500/40 hover:decoration-blue-400 transition-colors"
          >
            {match[5]}
          </a>
        );
      } else {
        const type = match[7];
        const val = match[8];
        if (type === '**') {
          parts.push(<strong key={match.index} className="font-semibold text-zinc-50">{val}</strong>);
        } else if (type === '~~') {
          parts.push(<del key={match.index} className="text-zinc-500 line-through">{val}</del>);
        } else if (type === '`') {
          parts.push(<code key={match.index} className="bg-zinc-800/80 px-1.5 py-0.5 rounded text-[13.5px] font-mono text-indigo-300 border border-zinc-700/50">{val}</code>);
        }
      }
      lastIdx = regex.lastIndex;
    }
    if (lastIdx < text.length) {
      parts.push(text.slice(lastIdx));
    }
    return parts;
  };

  const flushTable = (key: number) => {
    if (currentTable.length === 0) return;
    const hasHeader = currentTable.length > 1 && currentTable[1].every(cell => cell.trim().startsWith('-'));
    const headerRow = hasHeader ? currentTable[0] : null;
    const bodyRows = hasHeader ? currentTable.slice(2) : currentTable;

    blocks.push(
      <div
        key={`table-${key}`}
        className="overflow-x-auto my-6 shadow-lg bg-zinc-950/20"
        style={{
          border: '1px solid rgba(255, 255, 255, 0.12)',
          borderRadius: '12px',
          marginTop: '20px',
          marginBottom: '20px',
        }}
      >
        <table className="w-full text-left border-collapse text-[14.5px] leading-relaxed">
          {headerRow && (
            <thead className="bg-zinc-900/40">
              <tr>
                {headerRow.map((cell, idx) => (
                  <th
                    key={idx}
                    style={{
                      padding: '14px 20px',
                      borderBottom: '1px solid rgba(255, 255, 255, 0.15)',
                    }}
                    className="font-semibold text-zinc-300 text-[13px] uppercase tracking-wider"
                  >
                    {cell.trim()}
                  </th>
                ))}
              </tr>
            </thead>
          )}
          <tbody className="bg-zinc-950/5">
            {bodyRows.map((row, rIdx) => {
              const isLast = rIdx === bodyRows.length - 1;
              return (
                <tr
                  key={rIdx}
                  style={{
                    borderBottom: isLast ? 'none' : '1px solid rgba(255, 255, 255, 0.08)',
                  }}
                  className="hover:bg-zinc-800/10 transition-colors"
                >
                  {row.map((cell, cIdx) => (
                    <td
                      key={cIdx}
                      style={{
                        padding: '14px 20px',
                      }}
                      className="text-zinc-300 font-sans"
                    >
                      {renderInline(cell.trim())}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    );
    currentTable = [];
  };

  // Block-level accumulators
  let blockquoteLines: string[] = [];
  let orderedListItems: string[] = [];

  const flushBlockquote = (key: number) => {
    if (blockquoteLines.length === 0) return;
    blocks.push(
      <blockquote
        key={`bq-${key}`}
        className="border-l-2 border-indigo-500/50 pl-4 my-4 text-zinc-300 italic bg-indigo-500/5 rounded-r-lg py-2"
        style={{ marginTop: '12px', marginBottom: '12px' }}
      >
        {blockquoteLines.map((ln, i) => (
          <p key={i} className="text-[15px] leading-relaxed my-0.5">{renderInline(ln)}</p>
        ))}
      </blockquote>
    );
    blockquoteLines = [];
  };

  const flushOrderedList = (key: number) => {
    if (orderedListItems.length === 0) return;
    blocks.push(
      <ol key={`ol-${key}`} className="list-decimal ml-7 my-2.5 space-y-1.5">
        {orderedListItems.map((item, i) => (
          <li key={i} className="text-zinc-200 leading-[1.85] text-[15.5px] pl-1">
            {renderInline(item)}
          </li>
        ))}
      </ol>
    );
    orderedListItems = [];
  };

  const flushAll = (key: number) => {
    flushTable(key);
    flushBlockquote(key);
    flushOrderedList(key);
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    // ── Code blocks ──
    if (trimmed.startsWith('```')) {
      flushAll(i);
      if (inCodeBlock) {
        const codeText = codeBlockLines.join('\n');
        const highlight = codeBlockLang && codeText.length <= MAX_HIGHLIGHT_CHARS;
        blocks.push(
          <div
            key={`code-${i}`}
            className="relative group my-5"
            style={{ marginTop: '20px', marginBottom: '20px' }}
          >
            {codeBlockLang && (
              <div className="text-[10px] font-mono text-zinc-500 uppercase tracking-wider px-3 py-1 bg-zinc-900 border border-zinc-800/60 border-b-0 rounded-t-lg inline-block">
                {codeBlockLang}
              </div>
            )}
            {highlight ? (
              <div className={`border border-zinc-800/60 overflow-x-auto text-[13.5px] leading-relaxed ${codeBlockLang ? 'rounded-b-xl rounded-tr-xl' : 'rounded-xl'}`}>
                <SyntaxHighlighter
                  language={codeBlockLang}
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
                  {codeText}
                </SyntaxHighlighter>
              </div>
            ) : (
              <pre className={`p-4 bg-zinc-950 border border-zinc-800/60 text-zinc-400 font-mono text-[13.5px] overflow-x-auto leading-relaxed shadow-sm max-w-full ${
                codeBlockLang ? 'rounded-b-xl rounded-tr-xl' : 'rounded-xl'
              }`}>
                <code>{codeText}</code>
              </pre>
            )}
            <CodeCopyButton text={codeText} />
          </div>
        );
        codeBlockLines = [];
        codeBlockLang = '';
        inCodeBlock = false;
      } else {
        // Extract language from opening fence
        codeBlockLang = trimmed.slice(3).trim();
        inCodeBlock = true;
      }
      continue;
    }

    if (inCodeBlock) {
      codeBlockLines.push(line);
      continue;
    }

    // ── Tables ──
    const isTableRow = trimmed.startsWith('|') || (trimmed.includes('|') && currentTable.length > 0);
    if (isTableRow) {
      flushBlockquote(i);
      flushOrderedList(i);
      const cells = trimmed.split('|');
      if (trimmed.startsWith('|')) cells.shift();
      if (trimmed.endsWith('|')) cells.pop();
      currentTable.push(cells.map(c => c.trim()));
      continue;
    } else if (currentTable.length > 0) {
      flushTable(i);
    }

    // ── Blockquote ──
    if (trimmed.startsWith('> ')) {
      flushOrderedList(i);
      flushTable(i);
      blockquoteLines.push(trimmed.slice(2));
      continue;
    } else if (blockquoteLines.length > 0) {
      flushBlockquote(i);
    }

    // ── Ordered list (1. / 2. / etc.) ──
    const olMatch = trimmed.match(/^(\d+)\.\s+(.*)$/);
    if (olMatch) {
      flushBlockquote(i);
      flushTable(i);
      orderedListItems.push(olMatch[2]);
      continue;
    } else if (orderedListItems.length > 0) {
      flushOrderedList(i);
    }

    // ── Headings ──
    if (trimmed.startsWith('#')) {
      const match = trimmed.match(/^(#{1,6})\s+(.*)$/);
      if (match) {
        const level = match[1].length;
        const text = match[2];
        const headingClasses =
          level === 1 ? 'text-[21px] font-semibold text-zinc-100 mt-6 mb-3' :
          level === 2 ? 'text-[17.5px] font-semibold text-zinc-200 mt-5 mb-2.5 border-b border-zinc-800/40 pb-1.5' :
          'text-[15px] font-medium text-zinc-300 mt-4.5 mb-2';
        const headingStyle =
          level === 1 ? { marginTop: '24px', marginBottom: '12px' } :
          level === 2 ? { marginTop: '20px', marginBottom: '10px' } :
          { marginTop: '16px', marginBottom: '8px' };
        blocks.push(
          React.createElement(`h${level}`, { key: i, className: headingClasses, style: headingStyle }, renderInline(text))
        );
        continue;
      }
    }

    // ── Horizontal rules ──
    if (/^[-*_]{3,}\s*$/.test(trimmed)) {
      blocks.push(<hr key={i} className="border-zinc-800 my-6" />);
      continue;
    }

    // ── Task list items (- [ ] / - [x]) ──
    const taskMatch = trimmed.match(/^[-*]\s+\[([ xX])\]\s+(.*)$/);
    if (taskMatch) {
      flushBlockquote(i);
      flushTable(i);
      const checked = taskMatch[1].toLowerCase() === 'x';
      const depth = Math.min(4, Math.floor((line.length - line.trimStart().length) / 2));
      blocks.push(
        <div key={i} className="flex items-start gap-2.5 text-zinc-200 leading-[1.85] text-[15.5px] my-2 ml-5" style={{ marginTop: '4px', marginBottom: '4px', marginLeft: 20 + depth * 18 }}>
          {checked ? (
            <CheckCircle2 size={15} className="text-emerald-400 flex-shrink-0 mt-[7px]" />
          ) : (
            <Circle size={15} className="text-zinc-600 flex-shrink-0 mt-[7px]" />
          )}
          <span className={`flex-1 ${checked ? 'text-zinc-500 line-through' : ''}`}>{renderInline(taskMatch[2])}</span>
        </div>
      );
      continue;
    }

    // ── Unordered list ──
    if (trimmed.startsWith('- ') || trimmed.startsWith('* ')) {
      const text = trimmed.substring(2);
      const depth = Math.min(4, Math.floor((line.length - line.trimStart().length) / 2));
      blocks.push(
        <div
          key={i}
          className="flex gap-2.5 text-zinc-200 leading-[1.85] text-[15.5px]"
          style={{ marginTop: '6px', marginBottom: '6px', marginLeft: 20 + depth * 18 }}
        >
          <span className="text-zinc-500 font-bold select-none" style={{ color: depth > 0 ? '#a78bfa' : undefined }}>{depth > 0 ? '◦' : '•'}</span>
          <span className="flex-1">{renderInline(text)}</span>
        </div>
      );
      continue;
    }

    // ── Empty line ──
    if (!trimmed) {
      continue;
    }

    // ── Paragraph ──
    blocks.push(
      <p
        key={i}
        className="text-zinc-200 leading-[1.85] my-4 break-words"
        style={{ marginTop: '12px', marginBottom: '12px' }}
      >
        {renderInline(line)}
      </p>
    );
  }

  flushAll(lines.length);
  return <div className="markdown-body select-text leading-[1.85]">{blocks}</div>;
}
