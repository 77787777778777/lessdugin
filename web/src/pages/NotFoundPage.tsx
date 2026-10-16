// SPDX-License-Identifier: AGPL-3.0-or-later
import { Link } from "@tanstack/react-router";
import ui from "../styles/ui.module.css";

export function NotFoundPage() {
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Not found</h1>
      <p>There is no page at this address.</p>
      <Link to="/">Go home</Link>
    </div>
  );
}
