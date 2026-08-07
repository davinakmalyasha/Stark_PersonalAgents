import { useState, useEffect } from 'react';
import { X, Plus, Trash2, Shield, FolderGit, User } from 'lucide-react';
import { fetchProfile, saveProfile, fetchWorkspaceRules, saveWorkspaceRules } from '../../services/api';
import type { UserProfile, WorkspaceRules, WorkspaceInfo } from '../../types';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  activeWorkspace: WorkspaceInfo | null;
}

export default function SettingsModal({ isOpen, onClose, activeWorkspace }: Props) {
  const [activeTab, setActiveTab] = useState<'profile' | 'workspace'>('profile');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Profile Form State
  const [profileName, setProfileName] = useState('');
  const [globalRules, setGlobalRules] = useState<string[]>([]);
  const [newGlobalRule, setNewGlobalRule] = useState('');

  // Workspace Form State
  const [stackFrontend, setStackFrontend] = useState('');
  const [stackBackend, setStackBackend] = useState('');
  const [stackDatabase, setStackDatabase] = useState('');
  const [workspaceRules, setWorkspaceRules] = useState<string[]>([]);
  const [newWorkspaceRule, setNewWorkspaceRule] = useState('');

  // Load Data
  useEffect(() => {
    if (!isOpen) return;
    setError(null);
    setIsLoading(true);

    const loadData = async () => {
      try {
        const profile = await fetchProfile();
        setProfileName(profile.user_info?.name || '');
        setGlobalRules(profile.global_rules || []);

        if (activeWorkspace) {
          const rules = await fetchWorkspaceRules(activeWorkspace.id);
          const stack = rules.stack || {};
          setStackFrontend(Array.isArray(stack.frontend) ? stack.frontend.join(', ') : '');
          setStackBackend(Array.isArray(stack.backend) ? stack.backend.join(', ') : '');
          setStackDatabase(Array.isArray(stack.database) ? stack.database.join(', ') : '');
          setWorkspaceRules(rules.workspace_rules || []);
        }
      } catch (err: any) {
        setError(err.message || 'Failed to load settings');
      } finally {
        setIsLoading(false);
      }
    };

    loadData();
  }, [isOpen, activeWorkspace]);

  if (!isOpen) return null;

  // Save Profile Handler
  const handleSaveProfile = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const updatedProfile: UserProfile = {
        user_info: { name: profileName },
        global_rules: globalRules,
      };
      await saveProfile(updatedProfile);
      onClose();
    } catch (err: any) {
      setError(err.message || 'Failed to save profile');
    } finally {
      setIsLoading(false);
    }
  };

  // Save Workspace Rules Handler
  const handleSaveWorkspaceRules = async () => {
    if (!activeWorkspace) return;
    setIsLoading(true);
    setError(null);
    try {
      const updatedRules: WorkspaceRules = {
        stack: {
          frontend: stackFrontend.split(',').map((s) => s.trim()).filter(Boolean),
          backend: stackBackend.split(',').map((s) => s.trim()).filter(Boolean),
          database: stackDatabase.split(',').map((s) => s.trim()).filter(Boolean),
        },
        workspace_rules: workspaceRules,
      };
      await saveWorkspaceRules(activeWorkspace.id, updatedRules);
      onClose();
    } catch (err: any) {
      setError(err.message || 'Failed to save workspace rules');
    } finally {
      setIsLoading(false);
    }
  };

  // Rule Handlers
  const addGlobalRule = () => {
    if (!newGlobalRule.trim()) return;
    setGlobalRules([...globalRules, newGlobalRule.trim()]);
    setNewGlobalRule('');
  };

  const removeGlobalRule = (index: number) => {
    setGlobalRules(globalRules.filter((_, i) => i !== index));
  };

  const addWorkspaceRule = () => {
    if (!newWorkspaceRule.trim()) return;
    setWorkspaceRules([...workspaceRules, newWorkspaceRule.trim()]);
    setNewWorkspaceRule('');
  };

  const removeWorkspaceRule = (index: number) => {
    setWorkspaceRules(workspaceRules.filter((_, i) => i !== index));
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm animate-fade-in">
      <div className="w-[680px] h-[580px] flex flex-col bg-zinc-900 border border-zinc-800 rounded-xl overflow-hidden shadow-2xl relative">
        
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-zinc-800 bg-zinc-900/50">
          <div className="flex items-center gap-2">
            <span className="text-zinc-100 font-semibold text-base">System Settings</span>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800 transition-colors"
          >
            <X size={18} />
          </button>
        </div>

        {/* Tab switcher */}
        <div className="flex border-b border-zinc-800 bg-zinc-950/40">
          <button
            onClick={() => setActiveTab('profile')}
            className={`flex items-center gap-2 px-5 py-3 text-xs font-semibold uppercase tracking-wider border-b-2 transition-colors ${
              activeTab === 'profile'
                ? 'border-blue-500 text-zinc-100 bg-zinc-800/10'
                : 'border-transparent text-zinc-500 hover:text-zinc-300'
            }`}
          >
            <User size={13} />
            Profile & Global Rules
          </button>
          <button
            onClick={() => setActiveTab('workspace')}
            disabled={!activeWorkspace}
            className={`flex items-center gap-2 px-5 py-3 text-xs font-semibold uppercase tracking-wider border-b-2 transition-colors ${
              !activeWorkspace ? 'opacity-40 cursor-not-allowed' : ''
            } ${
              activeTab === 'workspace'
                ? 'border-blue-500 text-zinc-100 bg-zinc-800/10'
                : 'border-transparent text-zinc-500 hover:text-zinc-300'
            }`}
            title={!activeWorkspace ? 'Select a workspace first' : undefined}
          >
            <FolderGit size={13} />
            Workspace Rules
          </button>
        </div>

        {/* Main Panel Content */}
        <div className="flex-1 overflow-y-auto p-6">
          {error && (
            <div className="mb-4 px-4 py-2 bg-red-950/40 border border-red-800/60 rounded-lg text-xs text-red-400">
              {error}
            </div>
          )}

          {isLoading && (
            <div className="absolute inset-0 bg-zinc-900/50 flex items-center justify-center backdrop-blur-[1px] z-10">
              <div className="w-6 h-6 border-2 border-zinc-600 border-t-blue-500 rounded-full animate-spin"></div>
            </div>
          )}

          {activeTab === 'profile' && (
            <div className="space-y-6">
              {/* Profile details */}
              <div>
                <label className="block text-[11px] text-zinc-400 font-semibold uppercase tracking-wider mb-2">
                  Client / User Name
                </label>
                <input
                  type="text"
                  value={profileName}
                  onChange={(e) => setProfileName(e.target.value)}
                  disabled={isLoading}
                  placeholder="Your Name"
                  className="w-full px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-blue-500/50 outline-none transition-colors"
                />
              </div>

              {/* Global rules list */}
              <div>
                <label className="block text-[11px] text-zinc-400 font-semibold uppercase tracking-wider mb-2">
                  Global Rules / Context
                </label>
                <div className="space-y-2 max-h-[240px] overflow-y-auto mb-3 border border-zinc-800 bg-zinc-950/20 p-2 rounded-lg">
                  {globalRules.length === 0 && (
                    <div className="text-zinc-600 text-xs italic p-2 text-center">No global rules defined yet.</div>
                  )}
                  {globalRules.map((rule, idx) => (
                    <div key={idx} className="flex gap-2 items-start p-2 bg-zinc-950/60 border border-zinc-800/40 rounded-md group">
                      <Shield size={12} className="text-zinc-500 mt-0.5 flex-shrink-0" />
                      <span className="text-[11px] text-zinc-300 leading-relaxed flex-1">{rule}</span>
                      <button
                        onClick={() => removeGlobalRule(idx)}
                        disabled={isLoading}
                        className="text-zinc-600 hover:text-red-400 transition-colors opacity-0 group-hover:opacity-100 p-0.5 rounded"
                      >
                        <Trash2 size={11} />
                      </button>
                    </div>
                  ))}
                </div>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={newGlobalRule}
                    onChange={(e) => setNewGlobalRule(e.target.value)}
                    onKeyDown={(e) => e.key === 'Enter' && addGlobalRule()}
                    disabled={isLoading}
                    placeholder="Add a new global development rule..."
                    className="flex-1 px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-blue-500/50 outline-none transition-colors"
                  />
                  <button
                    onClick={addGlobalRule}
                    disabled={isLoading || !newGlobalRule.trim()}
                    className="px-3 py-2 bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 rounded-lg text-zinc-200 transition-colors"
                  >
                    <Plus size={14} />
                  </button>
                </div>
              </div>
            </div>
          )}

          {activeTab === 'workspace' && activeWorkspace && (
            <div className="space-y-6">
              {/* Tech Stack Fields */}
              <div>
                <label className="block text-[11px] text-zinc-400 font-semibold uppercase tracking-wider mb-2">
                  Workspace Tech Stack
                </label>
                <div className="grid grid-cols-3 gap-3">
                  <div>
                    <span className="text-[10px] text-zinc-500 block mb-1">Frontend</span>
                    <input
                      type="text"
                      value={stackFrontend}
                      onChange={(e) => setStackFrontend(e.target.value)}
                      disabled={isLoading}
                      placeholder="e.g. React, Vite"
                      className="w-full px-2.5 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-blue-500/50 outline-none"
                    />
                  </div>
                  <div>
                    <span className="text-[10px] text-zinc-500 block mb-1">Backend</span>
                    <input
                      type="text"
                      value={stackBackend}
                      onChange={(e) => setStackBackend(e.target.value)}
                      disabled={isLoading}
                      placeholder="e.g. Express, Node"
                      className="w-full px-2.5 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-blue-500/50 outline-none"
                    />
                  </div>
                  <div>
                    <span className="text-[10px] text-zinc-500 block mb-1">Database</span>
                    <input
                      type="text"
                      value={stackDatabase}
                      onChange={(e) => setStackDatabase(e.target.value)}
                      disabled={isLoading}
                      placeholder="e.g. PostgreSQL, SQLite"
                      className="w-full px-2.5 py-1.5 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-blue-500/50 outline-none"
                    />
                  </div>
                </div>
              </div>

              {/* Workspace specific rules list */}
              <div>
                <label className="block text-[11px] text-zinc-400 font-semibold uppercase tracking-wider mb-2">
                  Workspace Rules
                </label>
                <div className="space-y-2 max-h-[170px] overflow-y-auto mb-3 border border-zinc-800 bg-zinc-950/20 p-2 rounded-lg">
                  {workspaceRules.length === 0 && (
                    <div className="text-zinc-600 text-xs italic p-2 text-center">No local rules defined for this project.</div>
                  )}
                  {workspaceRules.map((rule, idx) => (
                    <div key={idx} className="flex gap-2 items-start p-2 bg-zinc-950/60 border border-zinc-800/40 rounded-md group">
                      <FolderGit size={12} className="text-zinc-500 mt-0.5 flex-shrink-0" />
                      <span className="text-[11px] text-zinc-300 leading-relaxed flex-1">{rule}</span>
                      <button
                        onClick={() => removeWorkspaceRule(idx)}
                        disabled={isLoading}
                        className="text-zinc-600 hover:text-red-400 transition-colors opacity-0 group-hover:opacity-100 p-0.5 rounded"
                      >
                        <Trash2 size={11} />
                      </button>
                    </div>
                  ))}
                </div>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={newWorkspaceRule}
                    onChange={(e) => setNewWorkspaceRule(e.target.value)}
                    onKeyDown={(e) => e.key === 'Enter' && addWorkspaceRule()}
                    disabled={isLoading}
                    placeholder="Add a new rule for this workspace..."
                    className="flex-1 px-3 py-2 bg-zinc-950 border border-zinc-800 rounded-lg text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-blue-500/50 outline-none transition-colors"
                  />
                  <button
                    onClick={addWorkspaceRule}
                    disabled={isLoading || !newWorkspaceRule.trim()}
                    className="px-3 py-2 bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 rounded-lg text-zinc-200 transition-colors"
                  >
                    <Plus size={14} />
                  </button>
                </div>
              </div>
            </div>
          )}
        </div>

        {/* Footer actions */}
        <div className="flex items-center justify-end gap-2 px-6 py-4 border-t border-zinc-800 bg-zinc-950/25">
          <button
            onClick={onClose}
            disabled={isLoading}
            className="px-4 py-2 text-xs font-semibold text-zinc-400 hover:text-zinc-200 rounded-lg hover:bg-zinc-800 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={activeTab === 'profile' ? handleSaveProfile : handleSaveWorkspaceRules}
            disabled={isLoading}
            className="px-5 py-2 text-xs font-semibold text-white bg-blue-600 hover:bg-blue-500 disabled:opacity-40 rounded-lg transition-colors"
          >
            Save Changes
          </button>
        </div>
      </div>
    </div>
  );
}
