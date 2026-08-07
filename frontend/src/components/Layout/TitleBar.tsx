interface Props {
}

export default function TitleBar(_: Props) {
  return (
    <header className="relative flex items-center justify-between select-none flex-shrink-0 h-[38px] bg-[#0c0c0e] border-b border-zinc-800/60 px-3 z-50" style={{ WebkitAppRegion: 'drag' } as any}>
    </header>
  );
}
