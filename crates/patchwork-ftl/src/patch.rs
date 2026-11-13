//! Applying mods' `.xml.append` files the way Slipstream does: the
//! "advanced XML" `mod:` tags find and change what's already in the file,
//! and everything else is added to its end.
//!
//! What a find looks through is the children of its context: the file's
//! top level (inside the `<FTL>` root tag the game's files have, if there
//! is one), or for a find nested in another, the element that one found.
//!
//! - `<mod:findName type="t" name="n">` finds elements named `t` (any, if
//!   left out) whose `name` attribute is `n`. By default it takes only the
//!   last one.
//! - `<mod:findLike type="t">` finds elements named `t` matching its
//!   `<mod:selector a="1">value</mod:selector>`: having those attributes,
//!   and that text if any is given.
//! - `<mod:findWithChildLike type="t" child-type="c">` finds elements named
//!   `t` with a child named `c` matching its selector.
//! - `<mod:findComposite>` takes the results of the finds in its
//!   `<mod:par op="AND|OR">`, which can nest.
//!
//! Every find takes `reverse` (search from the end), `start` (skip that
//! many matches), `limit` (`-1` for no limit) and `panic` (fail if nothing
//! matches; any value but `false` does, and one other than `true` is the
//! message). The commands inside a find act on each element it found, in
//! order: nested finds, `mod:setAttributes`, `mod:removeAttributes`,
//! `mod:setValue`, `mod:removeTag`, `<mod-append:x>` (add an `<x>` child)
//! and `<mod-overwrite:x>` (replace the first `<x>` child, or add one).

use crate::xml::{self, Element, Node};

/// The game's files' root tag, which Slipstream sees through.
const FTL: &str = "FTL";

/// A file being patched.
pub struct Doc {
    /// The file's top level, with an empty name. If the file has an
    /// `<FTL>` root, this has only that element (and comments and the
    /// like), and finds look inside it.
    root: Element,
}

/// Where in `nodes` the `<FTL>` root is, if they have one and no other
/// elements.
fn ftl_root(nodes: &[Node]) -> Option<usize> {
    let mut elements = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n, Node::Element(_)));
    match (elements.next(), elements.next()) {
        (Some((i, Node::Element(e))), None) if e.name == FTL => Some(i),
        _ => None,
    }
}

impl Doc {
    pub fn parse(text: &str) -> Self {
        let mut root = Element::new("");
        root.children = xml::parse(text);
        Self { root }
    }

    pub fn write(&self) -> String {
        xml::write(&self.root.children)
    }

    fn context(&mut self) -> &mut Element {
        match ftl_root(&self.root.children) {
            Some(i) => match &mut self.root.children[i] {
                Node::Element(e) => e,
                _ => unreachable!(),
            },
            None => &mut self.root,
        }
    }

    /// Applies an `.xml.append` file, ignoring any `<FTL>` tags around its
    /// parts. Anything that isn't serious enough
    /// to stop patching, like a command patchwork doesn't know, goes in
    /// `warnings`.
    pub fn append(&mut self, patch: &str, warnings: &mut Vec<String>) -> Result<(), String> {
        let mut nodes = Vec::new();
        unwrap_ftl(xml::parse(patch), &mut nodes);
        nodes.retain(|n| !matches!(n, Node::Other(o) if o.starts_with("<?xml")));
        let context = self.context();
        for node in nodes {
            match node {
                Node::Element(e) if e.name.starts_with("mod:find") => {
                    run_find(context, &e, warnings)?;
                }
                Node::Element(e) if is_command(&e.name) => {
                    if command(context, &e, warnings)? {
                        warnings.push("`mod:removeTag` outside a find was ignored".to_owned());
                    }
                }
                node => context.children.push(node),
            }
        }
        Ok(())
    }
}

/// `nodes` with any `<FTL>` tags at the top taken away, leaving what's in
/// them. Mods' patches can have several, even nested or left open, around
/// different parts.
fn unwrap_ftl(nodes: Vec<Node>, out: &mut Vec<Node>) {
    for node in nodes {
        match node {
            Node::Element(e) if e.name == FTL => unwrap_ftl(e.children, out),
            node => out.push(node),
        }
    }
}

/// Appends `patch` to `base` as text, as `.xml.rawappend` files are:
/// without parsing either, but keeping the `<FTL>` root (if `base` has one)
/// around both.
pub fn raw_append(base: &str, patch: &str) -> String {
    fn strip(text: &str) -> (&str, bool) {
        let mut text = text.trim_start();
        if text.starts_with("<?xml") {
            text = text.find("?>").map_or("", |i| &text[i + 2..]);
        }
        let open = format!("<{FTL}>");
        let close = format!("</{FTL}>");
        match (text.find(&open), text.rfind(&close)) {
            (Some(s), Some(e)) if s + open.len() <= e => (&text[s + open.len()..e], true),
            _ => (text, false),
        }
    }
    let (base_inner, wrapped) = strip(base);
    let (patch_inner, _) = strip(patch);
    if wrapped {
        format!("<{FTL}>{base_inner}\n{patch_inner}\n</{FTL}>\n")
    } else {
        format!("{base}\n{patch_inner}\n")
    }
}

fn is_command(name: &str) -> bool {
    name.starts_with("mod:")
        || name.starts_with("mod-append:")
        || name.starts_with("mod-overwrite:")
}

/// A find's `reverse`, `start`, `limit` and `panic`.
struct Bounds {
    reverse: bool,
    start: usize,
    limit: Option<usize>,
    /// The message to fail with when nothing's found.
    panic: Option<String>,
}

impl Bounds {
    fn of(find: &Element, reverse: bool, limit: Option<usize>) -> Result<Self, String> {
        let bool_attr = |name: &str, default: bool| match find.attr(name) {
            None => Ok(default),
            Some(v) => v.trim().parse::<bool>().map_err(|_| {
                format!(
                    "`{}` has {name}=\"{v}\", which isn't true or false",
                    find.name
                )
            }),
        };
        let int_attr = |name: &str| {
            find.attr(name)
                .map(|v| {
                    v.trim().parse::<i64>().map_err(|_| {
                        format!("`{}` has {name}=\"{v}\", which isn't a number", find.name)
                    })
                })
                .transpose()
        };
        let panic = match find.attr("panic").map(str::trim) {
            None | Some("false") => None,
            Some("true") => Some(format!("`{}` found nothing", describe(find))),
            Some(message) => Some(message.to_owned()),
        };
        Ok(Self {
            reverse: bool_attr("reverse", reverse)?,
            start: int_attr("start")?.map_or(0, |s| s.max(0) as usize),
            limit: match int_attr("limit")? {
                None => limit,
                Some(n) if n < 0 => None,
                Some(n) => Some(n as usize),
            },
            panic,
        })
    }

    /// Takes the indexes of the matches, in document order, and keeps the
    /// ones these bounds allow, in the order they're searched.
    fn apply(&self, mut found: Vec<usize>) -> Vec<usize> {
        if self.reverse {
            found.reverse();
        }
        found
            .into_iter()
            .skip(self.start)
            .take(self.limit.unwrap_or(usize::MAX))
            .collect()
    }
}

/// A find tag, for messages.
fn describe(find: &Element) -> String {
    let mut text = format!("<{}", find.name);
    for (name, value) in &find.attrs {
        text.push_str(&format!(" {name}=\"{value}\""));
    }
    text.push('>');
    text
}

/// The `<mod:selector>` of a find, as the attributes and (trimmed) text it
/// asks for.
fn selector(find: &Element) -> (Vec<(String, String)>, String) {
    match find.elements().find(|e| e.name == "mod:selector") {
        Some(s) => (s.attrs.clone(), s.text().trim().to_owned()),
        None => (Vec::new(), String::new()),
    }
}

fn matches(e: &Element, ty: Option<&str>, attrs: &[(String, String)], value: &str) -> bool {
    ty.is_none_or(|t| e.name == t)
        && attrs.iter().all(|(n, v)| e.attr(n) == Some(v))
        && (value.is_empty() || e.text().trim() == value)
}

/// The indexes of `context`'s children that `find` finds, in the order to
/// act on them.
fn find_in(context: &Element, find: &Element) -> Result<Vec<usize>, String> {
    let children = || {
        context
            .children
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                Node::Element(e) => Some((i, e)),
                _ => None,
            })
    };
    let ty = find.attr("type");
    let (found, bounds) = match find.name.as_str() {
        "mod:findName" => {
            let name = find
                .attr("name")
                .ok_or_else(|| format!("`{}` has no name to find", describe(find)))?;
            let found = children()
                .filter(|(_, e)| ty.is_none_or(|t| e.name == t) && e.attr("name") == Some(name))
                .map(|(i, _)| i)
                .collect();
            (found, Bounds::of(find, true, Some(1))?)
        }
        "mod:findLike" => {
            let (attrs, value) = selector(find);
            let found = children()
                .filter(|(_, e)| matches(e, ty, &attrs, &value))
                .map(|(i, _)| i)
                .collect();
            (found, Bounds::of(find, false, None)?)
        }
        "mod:findWithChildLike" => {
            let (attrs, value) = selector(find);
            let child_ty = find.attr("child-type");
            let found = children()
                .filter(|(_, e)| {
                    ty.is_none_or(|t| e.name == t)
                        && e.elements().any(|c| matches(c, child_ty, &attrs, &value))
                })
                .map(|(i, _)| i)
                .collect();
            (found, Bounds::of(find, false, None)?)
        }
        "mod:findComposite" => {
            let par = find
                .elements()
                .find(|e| e.name == "mod:par")
                .ok_or_else(|| format!("`{}` has no `<mod:par>`", describe(find)))?;
            (par_in(context, par)?, Bounds::of(find, false, None)?)
        }
        other => return Err(format!("`<{other}>` isn't a find patchwork knows")),
    };
    let found = bounds.apply(found);
    if found.is_empty()
        && let Some(message) = bounds.panic
    {
        return Err(message);
    }
    Ok(found)
}

/// The results of a `<mod:par>`'s finds, combined, in document order.
fn par_in(context: &Element, par: &Element) -> Result<Vec<usize>, String> {
    let and = match par.attr("op").map(str::trim) {
        Some("AND") | None => true,
        Some("OR") => false,
        Some(op) => return Err(format!("`<mod:par op=\"{op}\">` should be AND or OR")),
    };
    let mut result: Option<Vec<usize>> = None;
    for e in par.elements() {
        let mut found = if e.name == "mod:par" {
            par_in(context, e)?
        } else {
            find_in(context, e)?
        };
        found.sort_unstable();
        result = Some(match result {
            None => found,
            Some(r) if and => r
                .into_iter()
                .filter(|i| found.binary_search(i).is_ok())
                .collect(),
            Some(mut r) => {
                r.extend(found);
                r.sort_unstable();
                r.dedup();
                r
            }
        });
    }
    Ok(result.unwrap_or_default())
}

/// Runs `find` in `context`, applying its commands to what it finds.
fn run_find(
    context: &mut Element,
    find: &Element,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let found = find_in(context, find)?;
    let mut removed = Vec::new();
    for i in found {
        let Node::Element(target) = &mut context.children[i] else {
            unreachable!()
        };
        for cmd in find.children.iter() {
            let Node::Element(cmd) = cmd else { continue };
            if matches!(cmd.name.as_str(), "mod:selector" | "mod:par") {
                continue;
            }
            if !is_command(&cmd.name) {
                warnings.push(format!(
                    "`<{}>` inside `{}` isn't a command, so it was ignored",
                    cmd.name,
                    describe(find)
                ));
                continue;
            }
            if command(target, cmd, warnings)? {
                removed.push(i);
                break;
            }
        }
    }
    removed.sort_unstable();
    for i in removed.into_iter().rev() {
        context.children.remove(i);
    }
    Ok(())
}

/// Applies one command to `target`. Returns whether it removed the target,
/// which is for the caller to do.
fn command(
    target: &mut Element,
    cmd: &Element,
    warnings: &mut Vec<String>,
) -> Result<bool, String> {
    if let Some(name) = cmd.name.strip_prefix("mod-append:") {
        target.children.push(Node::Element(renamed(cmd, name)));
    } else if let Some(name) = cmd.name.strip_prefix("mod-overwrite:") {
        let new = Node::Element(renamed(cmd, name));
        let existing = target
            .children
            .iter()
            .position(|n| matches!(n, Node::Element(e) if e.name == name));
        match existing {
            Some(i) => target.children[i] = new,
            None => target.children.push(new),
        }
    } else {
        match cmd.name.as_str() {
            n if n.starts_with("mod:find") => run_find(target, cmd, warnings)?,
            "mod:setAttributes" => {
                for (name, value) in &cmd.attrs {
                    target.set_attr(name, value);
                }
            }
            "mod:removeAttributes" => {
                for (name, _) in &cmd.attrs {
                    target.remove_attr(name);
                }
            }
            "mod:setValue" => target.children = vec![Node::Text(cmd.text())],
            "mod:removeTag" => return Ok(true),
            other => warnings.push(format!(
                "`<{other}>` isn't a command patchwork knows, so it was ignored"
            )),
        }
    }
    Ok(false)
}

/// A copy of `e` called `name`.
fn renamed(e: &Element, name: &str) -> Element {
    Element {
        name: name.to_owned(),
        attrs: e.attrs.clone(),
        children: e.children.clone(),
    }
}
