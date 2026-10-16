// SPDX-License-Identifier: AGPL-3.0-or-later

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

/** Compact age like LessWrong's ("5m", "3h", "2d"), or a date beyond a month. Future
 * timestamps (claimed by the record) read "in the future". */
export function shortAge(ms: number, now: number = Date.now()): string {
  const d = now - ms;
  if (d < -MINUTE) return "in the future";
  if (d < MINUTE) return "now";
  if (d < HOUR) return `${Math.floor(d / MINUTE)}m`;
  if (d < DAY) return `${Math.floor(d / HOUR)}h`;
  if (d < 31 * DAY) return `${Math.floor(d / DAY)}d`;
  return dateFormat.format(ms);
}

export function fullDate(ms: number): string {
  return dateTimeFormat.format(ms);
}

export function isoDate(ms: number): string {
  return new Date(ms).toISOString();
}
