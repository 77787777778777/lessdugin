// SPDX-License-Identifier: AGPL-3.0-or-later
export const DIST: string;
export const HEADERS_FILE: string;
export function parseHeadersFile(text: string): Record<string, Record<string, string>>;
export function productionHeaders(): Promise<Record<string, string>>;
export function startStaticServer(opts?: { port?: number; root?: string }): Promise<{
  url: string;
  headers: Record<string, string>;
  close: () => Promise<void>;
}>;
