// Profiles kept in step with other machines. main.rs syncs them in the
// background and sends patchwork://synced whenever one changes; it's
// shown as it is now, but never deployed by itself.

import { listen, state, status } from "./state.js";
import { refresh } from "./profile.js";

listen("patchwork://synced", ({ payload: e }) => {
  for (const warning of e.warnings) status(warning, true);
  if (e.changed) {
    if (e.profile && e.profile === state.profile?.id) {
      status("Updated from another machine; deploy to apply the changes");
    }
    refresh().catch(() => {});
  }
});
