import { FolderOpen, FolderPlus, X } from 'lucide-react';

interface Props {
  isOpen: boolean;
  value: string;
  name: string;
  onChange: (value: string) => void;
  onNameChange: (value: string) => void;
  onBrowse: () => void;
  onClose: () => void;
  onSubmit: () => void;
}

export default function SidebarAddFolderModal({ isOpen, value, name, onChange, onNameChange, onBrowse, onClose, onSubmit }: Props) {
  if (!isOpen) return null;
  return <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/65 backdrop-blur-sm" onClick={onClose}>
    <div className="w-[720px] max-w-[calc(100vw-32px)] rounded-[20px] border border-white/10 bg-[#29292c] p-7 shadow-2xl" onClick={(event) => event.stopPropagation()}>
      <div className="mb-6 flex items-center justify-between"><h3 className="text-[24px] font-semibold text-zinc-100">Create project</h3><button onClick={onClose} className="text-zinc-500 hover:text-zinc-200"><X size={19}/></button></div>
      <div className="relative mb-6"><FolderOpen size={17} className="absolute left-4 top-1/2 -translate-y-1/2 text-zinc-400"/><input value={name} onChange={(event) => onNameChange(event.target.value)} placeholder="Project name" autoFocus className="w-full rounded-2xl border border-zinc-600 bg-[#252528] py-3.5 pl-11 pr-4 text-[15px] text-zinc-100 outline-none placeholder:text-zinc-500 focus:border-sky-300"/></div>
      <label className="mb-3 block text-[14px] font-medium text-zinc-200">Source folders</label>
      <button onClick={onBrowse} className="flex h-36 w-full flex-col items-center justify-center gap-3 rounded-2xl border border-white/[.09] bg-black/[.08] text-[16px] text-zinc-300 transition-colors hover:bg-white/[.035] hover:text-white"><FolderPlus size={22} className="text-zinc-400"/><span>{value ? value : 'Add folders the agent can read and edit'}</span></button>
      <input value={value} onChange={(event) => onChange(event.target.value)} placeholder="Or paste a folder path" className="mt-4 w-full border-b border-zinc-700 bg-transparent px-1 py-3 text-[13px] text-zinc-400 outline-none placeholder:text-zinc-600 focus:border-zinc-500"/>
      <div className="mt-8 flex justify-end gap-4 border-t border-white/[.08] pt-5"><button onClick={onClose} className="px-4 py-2.5 text-[15px] text-zinc-400 hover:text-zinc-200">Cancel</button><button onClick={onSubmit} disabled={!value.trim()} className="rounded-xl bg-zinc-100 px-5 py-2.5 text-[15px] font-medium text-zinc-900 transition hover:bg-white disabled:cursor-not-allowed disabled:opacity-40">Create project</button></div>
    </div>
  </div>;
}
