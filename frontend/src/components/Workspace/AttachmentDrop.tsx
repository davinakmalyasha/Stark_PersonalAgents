/* eslint-disable react-refresh/only-export-components */
import {
  createContext, forwardRef, useCallback, useContext, useEffect, useMemo, useRef, useState,
} from 'react';
import { createPortal } from 'react-dom';
import type { DragEvent as ReactDragEvent, ReactNode } from 'react';
import { AudioLines, FileText, Image, UploadCloud } from 'lucide-react';

type AttachFiles = (files: File[]) => void;

interface DropContextValue {
  register: (handler: AttachFiles) => () => void;
  attach: AttachFiles;
}

const DropContext = createContext<DropContextValue | null>(null);

export function AttachmentDropProvider({ children }: { children: ReactNode }) {
  const handlers = useRef(new Set<AttachFiles>());
  const register = useCallback((handler: AttachFiles) => {
    handlers.current.add(handler);
    return () => { handlers.current.delete(handler); };
  }, []);
  const attach = useCallback((files: File[]) => {
    if (!files.length) return;
    for (const handler of handlers.current) handler(files);
  }, []);
  const value = useMemo(() => ({ register, attach }), [register, attach]);
  return <DropContext.Provider value={value}>{children}</DropContext.Provider>;
}

export function useAttachmentDrop(): DropContextValue {
  const value = useContext(DropContext);
  if (!value) throw new Error('useAttachmentDrop must be used inside AttachmentDropProvider');
  return value;
}

export function useOptionalAttachmentDrop(): DropContextValue | null {
  return useContext(DropContext);
}

function hasFiles(event: ReactDragEvent): boolean {
  return Array.from(event.dataTransfer.types || []).includes('Files');
}

export interface DropOverlayProps {
  compact?: boolean;
  count?: number;
  title?: string;
}

export function DropOverlay({ compact = false, count = 0, title }: DropOverlayProps) {
  return (
    <div className={`file-drop-overlay ${compact ? 'is-compact' : ''}`} aria-hidden="true">
      <div className="file-drop-frame">
        <div className="file-drop-orbit">
          <UploadCloud size={compact ? 18 : 24} />
          <span className="file-drop-orbit-ring" />
        </div>
        <div className="file-drop-copy">
          <strong>{title || (compact ? 'Drop to attach' : 'Drop files to attach')}</strong>
          {!compact && (
            <span>
              {count > 0 ? `${count} file${count === 1 ? '' : 's'} ready` : 'Images, audio, documents, and code · up to 20 MB each'}
            </span>
          )}
        </div>
        {!compact && (
          <div className="file-drop-types">
            <span><Image size={11} /> Images</span>
            <span><AudioLines size={11} /> Audio</span>
            <span><FileText size={11} /> Files</span>
          </div>
        )}
      </div>
    </div>
  );
}

interface SurfaceProps {
  children: ReactNode;
  className?: string;
  compact?: boolean;
  title?: string;
}

/**
 * A full chat-panel drop target. The visual overlay is portalled so the
 * scroll container can keep its own overflow; the portal ignores pointer
 * events, letting drops reach this surface.
 */
export const FileDropSurface = forwardRef<HTMLDivElement, SurfaceProps>(function FileDropSurface(
  { children, className = '', compact = false, title },
  ref,
) {
  const ownRef = useRef<HTMLDivElement | null>(null);
  const depth = useRef(0);
  const [active, setActive] = useState(false);
  const [count, setCount] = useState(0);
  const [rect, setRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  const { attach } = useAttachmentDrop();

  const setRefs = useCallback((node: HTMLDivElement | null) => {
    ownRef.current = node;
    if (typeof ref === 'function') ref(node);
    else if (ref) ref.current = node;
  }, [ref]);

  const updateRect = useCallback(() => {
    const node = ownRef.current;
    if (!node) return;
    const box = node.getBoundingClientRect();
    setRect({ left: box.left, top: box.top, width: box.width, height: box.height });
  }, []);

  useEffect(() => {
    if (!active) return;
    updateRect();
    const node = ownRef.current;
    if (!node) return;
    const observer = new ResizeObserver(updateRect);
    observer.observe(node);
    window.addEventListener('resize', updateRect);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', updateRect);
    };
  }, [active, updateRect]);

  const onDragEnter = (event: ReactDragEvent<HTMLDivElement>) => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    depth.current += 1;
    setCount(event.dataTransfer.items?.length || 0);
    setActive(true);
  };
  const onDragOver = (event: ReactDragEvent<HTMLDivElement>) => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'copy';
    setCount(event.dataTransfer.items?.length || count);
  };
  const onDragLeave = (event: ReactDragEvent<HTMLDivElement>) => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    depth.current -= 1;
    if (depth.current <= 0) {
      depth.current = 0;
      setActive(false);
      setCount(0);
    }
  };
  const onDrop = (event: ReactDragEvent<HTMLDivElement>) => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    depth.current = 0;
    setActive(false);
    const files = Array.from(event.dataTransfer.files || []);
    attach(files);
    setCount(0);
  };

  return (
    <div
      ref={setRefs}
      className={className}
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {children}
      {active && rect && typeof document !== 'undefined' && createPortal(
        <div
          className="file-drop-portal"
          style={{ left: rect.left, top: rect.top, width: rect.width, height: rect.height }}
        >
          <DropOverlay compact={compact} count={count} title={title} />
        </div>,
        document.body,
      )}
    </div>
  );
});
