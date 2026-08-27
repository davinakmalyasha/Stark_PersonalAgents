import type { TaskRecurrence } from '../types';

const FIXED_LABELS: Record<Exclude<TaskRecurrence, `interval-${number}` | 'none'>, string> = {
  daily: 'Daily',
  weekly: 'Weekly',
  monthly: 'Monthly',
};

export const parseIntervalMinutes = (recurrence: TaskRecurrence): number | null => {
  if (!recurrence || !recurrence.startsWith('interval-')) return null;
  const minutes = Number(recurrence.slice('interval-'.length));
  return Number.isFinite(minutes) && minutes > 0 ? minutes : null;
};

export const formatRecurrence = (recurrence: TaskRecurrence | string | undefined | null): string => {
  if (!recurrence || recurrence === 'none') return '';
  if (recurrence in FIXED_LABELS) return FIXED_LABELS[recurrence as keyof typeof FIXED_LABELS];
  const minutes = parseIntervalMinutes(recurrence as TaskRecurrence);
  if (minutes == null) return '';
  if (minutes % 1440 === 0) {
    const days = minutes / 1440;
    return `Every ${days} day${days === 1 ? '' : 's'}`;
  }
  if (minutes % 60 === 0) {
    const hours = minutes / 60;
    return `Every ${hours} hour${hours === 1 ? '' : 's'}`;
  }
  return `Every ${minutes} min`;
};

export interface CustomInterval {
  amount: number;
  unit: 'minutes' | 'hours' | 'days';
}

export const customIntervalFromRecurrence = (recurrence: TaskRecurrence | undefined): CustomInterval | null => {
  const minutes = recurrence ? parseIntervalMinutes(recurrence) : null;
  if (minutes == null) return null;
  if (minutes % 1440 === 0) return { amount: minutes / 1440, unit: 'days' };
  if (minutes % 60 === 0) return { amount: minutes / 60, unit: 'hours' };
  return { amount: minutes, unit: 'minutes' };
};

export const recurrenceFromCustomInterval = (interval: CustomInterval | null): TaskRecurrence => {
  if (!interval || interval.amount <= 0) return 'none';
  const minutes = interval.unit === 'days' ? interval.amount * 1440 : interval.unit === 'hours' ? interval.amount * 60 : interval.amount;
  return `interval-${Math.round(minutes)}` as TaskRecurrence;
};