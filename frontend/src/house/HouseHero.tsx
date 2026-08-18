function greeting(): string {
  const h = new Date().getHours();
  if (h < 5) return 'Still up?';
  if (h < 12) return 'Good morning';
  if (h < 17) return 'Good afternoon';
  if (h < 22) return 'Good evening';
  return 'Still up?';
}

export default function HouseHero() {
  return (
    <div className="flex flex-col items-center justify-center text-center select-none px-6">
      <p className="house-rise-1 text-[13px] text-zinc-500 mb-1.5">{greeting()}</p>
      <h1 className="house-rise-2 house-font-display text-[28px] sm:text-[34px] font-bold tracking-tight text-white">
        How can I help you today?
      </h1>
      <div className="house-rise-3 mt-2.5 flex items-center justify-center gap-1.5">
        <span className="house-status-dot !w-1 !h-1" />
        <span className="text-[11px] text-zinc-500 tracking-wide">VIN online — general memory active</span>
      </div>
    </div>
  );
}
