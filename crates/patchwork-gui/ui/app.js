// Patchwork's UI. Plain JS modules with no build step: everything goes
// through invoke() to the commands in main.rs.
//
// Mod names, summaries and so on come from remote sources, so they're only
// ever put in the page as text (see h() in dom.js), never as HTML. The one
// exception is changelogs, which main.rs turns from markdown into HTML with
// anything unsafe taken out (see changelog.rs).

import { modsView, setModsView } from "./mods.js";
import { refresh } from "./profile.js";
// these only wire up their parts of the page
import "./games.js";
import "./browse.js";
import "./plugins.js";
import "./toolbar.js";
import "./deploy.js";
import "./history.js";
import "./play.js";
import "./downloads.js";
import "./selfupdate.js";
import "./cli.js";
import "./settings.js";
import "./sync.js";

setModsView(modsView);
refresh().catch(() => {});
