// SPDX-License-Identifier: AGPL-3.0-or-later
export const PRODUCTION_CSP_DIRECTIVES: readonly string[];
export const PRODUCTION_CSP: string;
export const DEV_CSP: string;
export const META_CSP: string;
export function injectMetaCsp(html: string): string;
