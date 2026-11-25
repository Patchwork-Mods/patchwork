//! The module descriptions Baldur's Gate 3 works with: each mod's
//! `Mods/<folder>/meta.lsx`, and the `modsettings.lsx` that lists the ones
//! to load.

use serde::Deserialize;

/// A module, as `meta.lsx` describes it and `modsettings.lsx` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    pub uuid: String,
    pub folder: String,
    pub name: String,
    #[serde(default)]
    pub version64: i64,
    #[serde(default)]
    pub publish_handle: u64,
    /// What the in-game mod manager checks its downloads against. Mods'
    /// own `meta.lsx` don't have one.
    #[serde(default)]
    pub md5: String,
    #[serde(skip)]
    pub dependencies: Vec<Dependency>,
}

/// A module another one needs loaded before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub uuid: String,
    pub name: String,
}

/// Reads `meta.lsx`: its `ModuleInfo` node, and the `ModuleShortDesc`s in
/// its `Dependencies`.
pub fn parse(text: &str) -> Result<Module, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let node = |id: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name("node") && n.attribute("id") == Some(id))
    };
    let info = node("ModuleInfo").ok_or("it has no ModuleInfo")?;
    let uuid = attribute(info, "UUID").ok_or("its ModuleInfo has no UUID")?;
    let folder = attribute(info, "Folder").ok_or("its ModuleInfo has no Folder")?;
    let name = attribute(info, "Name").unwrap_or_else(|| folder.clone());
    // Older mods have a 32-bit `Version` rather than `Version64`.
    let version64 = version_of(info)?;
    let publish_handle = publish_handle(info);
    let dependencies = node("Dependencies")
        .map(|deps| {
            deps.descendants()
                .filter(|n| n.attribute("id") == Some("ModuleShortDesc"))
                .filter_map(|n| {
                    let uuid = attribute(n, "UUID").filter(|u| !u.is_empty())?;
                    let name = attribute(n, "Name")
                        .or_else(|| attribute(n, "Folder"))
                        .unwrap_or_else(|| uuid.clone());
                    Some(Dependency { uuid, name })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Module {
        uuid,
        folder,
        name,
        version64,
        publish_handle,
        md5: String::new(),
        dependencies,
    })
}

/// A module's `Version64`, or its older 32-bit `Version` widened.
fn version_of(node: roxmltree::Node<'_, '_>) -> Result<i64, String> {
    Ok(match attribute(node, "Version64") {
        Some(v) => v
            .parse()
            .map_err(|_| format!("its Version64 `{v}` isn't a number"))?,
        None => attribute(node, "Version")
            .and_then(|v| v.parse::<u32>().ok())
            .map_or(0, widen),
    })
}

fn publish_handle(node: roxmltree::Node<'_, '_>) -> u64 {
    attribute(node, "PublishHandle")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// The modules a `modsettings.lsx` loads, in order: the `ModuleShortDesc`s
/// in its `Mods` node.
pub fn parse_modsettings(text: &str) -> Result<Vec<Module>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let Some(mods) = doc
        .descendants()
        .find(|n| n.has_tag_name("node") && n.attribute("id") == Some("Mods"))
    else {
        return Ok(Vec::new());
    };
    mods.descendants()
        .filter(|n| n.has_tag_name("node") && n.attribute("id") == Some("ModuleShortDesc"))
        .filter_map(|n| {
            let uuid = attribute(n, "UUID").filter(|u| !u.is_empty())?;
            Some((n, uuid))
        })
        .map(|(n, uuid)| {
            let folder = attribute(n, "Folder").unwrap_or_default();
            Ok(Module {
                name: attribute(n, "Name").unwrap_or_else(|| folder.clone()),
                folder,
                version64: version_of(n).map_err(|why| format!("{uuid}: {why}"))?,
                publish_handle: publish_handle(n),
                md5: attribute(n, "MD5").unwrap_or_default(),
                uuid,
                dependencies: Vec::new(),
            })
        })
        .collect()
}

/// The value of a node's `<attribute id="…" value="…"/>` child.
fn attribute(node: roxmltree::Node<'_, '_>, id: &str) -> Option<String> {
    node.children()
        .find(|c| c.has_tag_name("attribute") && c.attribute("id") == Some(id))
        .and_then(|c| c.attribute("value"))
        .map(|v| v.trim().to_owned())
}

/// A 32-bit version (4 bits major, 4 minor, 8 revision, 16 build) as a
/// 64-bit one (9 bits major, 8 minor, 16 revision, 31 build).
fn widen(v: u32) -> i64 {
    let (major, minor, revision, build) = (v >> 28, (v >> 24) & 0xf, (v >> 16) & 0xff, v & 0xffff);
    ((major as i64) << 55) | ((minor as i64) << 47) | ((revision as i64) << 31) | build as i64
}

/// A 64-bit version as people write it: `1.2.0.5`.
pub fn version(v: i64) -> String {
    let v = v as u64;
    format!(
        "{}.{}.{}.{}",
        v >> 55,
        (v >> 47) & 0xff,
        (v >> 31) & 0xffff,
        v & 0x7fff_ffff
    )
}

/// `modsettings.lsx` loading `modules`, in order, as BG3 Mod Manager
/// writes it for the game since patch 7: no `ModOrder` node, just `Mods`.
pub fn modsettings(modules: &[&Module]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <save>\n\
         \x20   <version major=\"4\" minor=\"8\" revision=\"0\" build=\"100\"/>\n\
         \x20   <region id=\"ModuleSettings\">\n\
         \x20       <node id=\"root\">\n\
         \x20           <children>\n\
         \x20               <node id=\"Mods\">\n\
         \x20                   <children>\n",
    );
    for m in modules {
        let attr = |id: &str, kind: &str, value: &str| {
            format!(
                "                            <attribute id=\"{id}\" type=\"{kind}\" value=\"{}\"/>\n",
                escape(value)
            )
        };
        out.push_str("                        <node id=\"ModuleShortDesc\">\n");
        out.push_str(&attr("Folder", "LSString", &m.folder));
        out.push_str(&attr("MD5", "LSString", &m.md5));
        out.push_str(&attr("Name", "LSString", &m.name));
        out.push_str(&attr(
            "PublishHandle",
            "uint64",
            &m.publish_handle.to_string(),
        ));
        out.push_str(&attr("UUID", "guid", &m.uuid));
        out.push_str(&attr("Version64", "int64", &m.version64.to_string()));
        out.push_str("                        </node>\n");
    }
    out.push_str(
        "                    </children>\n\
         \x20               </node>\n\
         \x20           </children>\n\
         \x20       </node>\n\
         \x20   </region>\n\
         </save>\n",
    );
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
