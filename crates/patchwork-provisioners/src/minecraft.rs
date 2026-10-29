//! Minecraft: Java Edition, as Mojang's launcher sets it up, with a mod
//! loader on top.
//!
//! Mojang describes each version of the game in a JSON file: its libraries,
//! its client jar, its assets (sounds and languages) and its arguments.
//! Mod loaders whose metadata servers speak Fabric's API (Fabric and
//! Quilt, and others like Legacy Fabric) give a profile in the same form
//! that inherits from the vanilla one, adding libraries and replacing the
//! main class. Which loaders those are is up to the game definition:
//!
//! ```toml
//! [provision]
//! kind = "minecraft"
//! # Optional; Mojang's by default.
//! manifest = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
//! # Where the Java runtimes Mojang's launcher uses are listed. Optional;
//! # Mojang's by default, and `false` runs the game with the system's Java.
//! java_runtimes = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json"
//!
//! # A pick of the `loader` axis, and the API its metadata is under.
//! # Picks that aren't here (or `vanilla`) get the game as it is.
//! [provision.loaders.fabric]
//! meta = "https://meta.fabricmc.net/v2"
//! ```
//!
//! The `version` axis is the Minecraft version, and a `loader-version`
//! axis, if the profile picks it, the loader's. Without one, the newest
//! stable version of the loader is used.

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use patchwork_core::deploy::Os;
use patchwork_core::provision::{
    JavaLaunch, Launch, ProvisionContext, Provisioner, RuntimeFile, Setup, Unpack,
};
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const KIND: &str = "minecraft";
const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RESOURCES: &str = "https://resources.download.minecraft.net";
const LIBRARIES: &str = "https://libraries.minecraft.net/";
const JAVA_RUNTIMES: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
/// The Java runtime for versions from before they said which they need.
const LEGACY_JAVA: &str = "jre-legacy";

/// The `[env]` axis with the mod loader in it.
const LOADER: &str = "loader";
/// The `[env]` axis with the mod loader's version in it.
const LOADER_VERSION: &str = "loader-version";

pub struct MinecraftProvisioner;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    manifest: Option<String>,
    #[serde(default)]
    java_runtimes: Option<JavaRuntimes>,
    #[serde(default)]
    loaders: BTreeMap<String, LoaderDef>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JavaRuntimes {
    /// Whether to use Mojang's.
    Mojang(bool),
    Url(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoaderDef {
    /// The root of its Fabric-style metadata API.
    meta: String,
}

#[async_trait]
impl Provisioner for MinecraftProvisioner {
    fn kind(&self) -> &str {
        KIND
    }

    async fn plan(&self, ctx: &ProvisionContext<'_>) -> Result<Setup> {
        let config: Config = toml::Value::Table(ctx.config.clone())
            .try_into()
            .map_err(|e| error(format!("its settings in the game definition: {e}")))?;
        let Some(version) = &ctx.env.version else {
            return Err(error(format!(
                "pick a Minecraft version first, as in `patchwork env {} 1.21.1`",
                patchwork_core::game::EnvDefs::VERSION
            )));
        };
        let loader = ctx.env.axes.get(LOADER).filter(|l| *l != "vanilla");
        let loader = match loader {
            None => None,
            Some(loader) => {
                let def = config.loaders.get(loader).ok_or_else(|| {
                    error(format!(
                        "can't set {loader} up yet; pick another mod loader, or set the game \
                         up in a launcher like Prism and point patchwork at it"
                    ))
                })?;
                let meta = def.meta.trim_end_matches('/');
                let picked = ctx.env.axes.get(LOADER_VERSION);
                let loader_version = match picked {
                    Some(v) => v.clone(),
                    None => newest_loader(loader, meta, version).await?,
                };
                let url = format!("{meta}/versions/loader/{version}/{loader_version}/profile/json");
                Some(get_json::<VersionJson>(&url).await?)
            }
        };

        let manifest_url = config.manifest.as_deref().unwrap_or(MANIFEST);
        let manifest: Manifest = get_json(manifest_url).await?;
        let listed = manifest
            .versions
            .iter()
            .find(|v| v.id == *version)
            .ok_or_else(|| error(format!("Mojang has no Minecraft version `{version}`")))?;
        let vanilla: VersionJson = get_json(&listed.url).await?;
        let index_ref = vanilla.asset_index.as_ref().ok_or_else(|| {
            error(format!(
                "Minecraft {version} doesn't say what its assets are"
            ))
        })?;
        let index: AssetIndex = get_json(&index_ref.url).await?;
        let runtimes = match &config.java_runtimes {
            None | Some(JavaRuntimes::Mojang(true)) => Some(JAVA_RUNTIMES),
            Some(JavaRuntimes::Mojang(false)) => None,
            Some(JavaRuntimes::Url(url)) => Some(url.as_str()),
        };
        let component = vanilla
            .java_version
            .as_ref()
            .and_then(|j| j.component.clone())
            .unwrap_or_else(|| LEGACY_JAVA.to_owned());
        let java = match runtimes {
            Some(url) => java_runtime(url, ctx.os, ctx.arch, &component).await?,
            None => None,
        };
        let mut setup = assemble(ctx.os, ctx.arch, vanilla, loader, &index)?;
        if let Some((files, exe)) = java {
            setup.files.extend(files);
            let Launch::Java(launch) = &mut setup.launch;
            launch.java_exe = Some(exe);
        }
        Ok(setup)
    }
}

/// The files of Mojang's Java runtime `component` for `os` and `arch`,
/// from the list at `url`, and which of them is Java. `None` if there
/// isn't one for them, so the system's Java has to do.
async fn java_runtime(
    url: &str,
    os: Os,
    arch: &str,
    component: &str,
) -> Result<Option<(Vec<RuntimeFile>, RelPath)>> {
    let Some(platform) = java_platform(os, arch) else {
        return Ok(None);
    };
    let mut runtimes: BTreeMap<String, BTreeMap<String, Vec<JavaRuntime>>> = get_json(url).await?;
    let Some(runtime) = runtimes
        .get_mut(platform)
        .and_then(|p| p.remove(component))
        .and_then(|r| r.into_iter().next())
    else {
        return Ok(None);
    };
    let manifest: JavaManifest = get_json(&runtime.manifest.url).await?;
    let dir = format!("java/{component}/{}", runtime.version.name);
    java_files(&manifest, &dir, os).map(Some)
}

/// Mojang's name for the platform of `os` and `arch`, among Java runtimes.
fn java_platform(os: Os, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        (Os::Linux, "x86_64") => "linux",
        (Os::Linux, "x86") => "linux-i386",
        (Os::Macos, "x86_64") => "mac-os",
        (Os::Macos, "aarch64") => "mac-os-arm64",
        (Os::Windows, "x86_64") => "windows-x64",
        (Os::Windows, "x86") => "windows-x86",
        (Os::Windows, "aarch64") => "windows-arm64",
        _ => return None,
    })
}

/// The files of a Java runtime from its `manifest`, to go in `dir`, and
/// which of them is Java. Links in it are left out; they're only to
/// license files.
fn java_files(manifest: &JavaManifest, dir: &str, os: Os) -> Result<(Vec<RuntimeFile>, RelPath)> {
    let mut files = Vec::new();
    for (path, entry) in &manifest.files {
        let JavaEntry::File {
            downloads,
            executable,
        } = entry
        else {
            continue;
        };
        files.push(RuntimeFile {
            path: rel(&format!("{dir}/{path}"))?,
            url: downloads.raw.url.clone(),
            size: downloads.raw.size,
            sha1: downloads.raw.sha1.clone(),
            unpack: None,
            executable: *executable,
        });
    }
    let candidates: &[&str] = match os {
        Os::Windows => &["bin/javaw.exe", "bin/java.exe"],
        Os::Macos => &["jre.bundle/Contents/Home/bin/java", "bin/java"],
        Os::Linux => &["bin/java"],
    };
    let exe = candidates
        .iter()
        .find(|c| manifest.files.contains_key(**c))
        .ok_or_else(|| error(format!("the Java runtime for {dir} has no Java in it")))?;
    Ok((files, rel(&format!("{dir}/{exe}"))?))
}

/// The newest stable version of `loader` for Minecraft `version`, from
/// its metadata at `meta`.
async fn newest_loader(loader: &str, meta: &str, version: &str) -> Result<String> {
    let url = format!("{meta}/versions/loader/{version}");
    // Fabric's says a version it's never heard of is a bad request.
    let versions = match get_json::<Vec<LoaderVersions>>(&url).await {
        Err(Error::Reqwest { result, .. })
            if result.status().is_some_and(|s| s.is_client_error()) =>
        {
            Vec::new()
        }
        versions => versions?,
    };
    pick_loader(&versions).ok_or_else(|| {
        error(format!(
            "{loader} doesn't have a version for Minecraft {version}"
        ))
    })
}

/// The newest stable one of `versions` (newest first, as metadata lists
/// them). Loaders that don't say which are stable (Quilt) mark the others
/// with a `-`, as in `0.27.0-beta.1`.
fn pick_loader(versions: &[LoaderVersions]) -> Option<String> {
    versions
        .iter()
        .map(|v| &v.loader)
        .find(|l| l.stable.unwrap_or(!l.version.contains('-')))
        .map(|l| l.version.clone())
}

/// Puts together the setup of `vanilla`, with `loader`'s profile (which
/// inherits from it) on top, if there is one.
fn assemble(
    os: Os,
    arch: &str,
    vanilla: VersionJson,
    loader: Option<VersionJson>,
    index: &AssetIndex,
) -> Result<Setup> {
    let id = loader.as_ref().map_or(&vanilla.id, |l| &l.id).clone();
    if let Some(loader) = &loader
        && loader
            .inherits_from
            .as_deref()
            .is_some_and(|v| v != vanilla.id)
    {
        return Err(error(format!(
            "{} is for Minecraft {}, not {}",
            loader.id,
            loader.inherits_from.as_deref().unwrap_or_default(),
            vanilla.id
        )));
    }
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
    {
        return Err(error(format!("`{id}` can't be the name of a setup")));
    }
    let natives = rel(&format!("natives/{id}"))?;
    let mut files = Vec::new();
    let mut classpath = Vec::new();

    // A loader's libraries come first, and take the place of the game's
    // own of the same name. A version can list a library more than once
    // (once for its jar and once for its natives), so within one they're
    // only told apart by file.
    let replaced: HashSet<String> = loader
        .iter()
        .flat_map(|l| &l.libraries)
        .map(Library::unversioned)
        .collect();
    let libraries = loader.iter().flat_map(|l| &l.libraries).chain(
        vanilla
            .libraries
            .iter()
            .filter(|l| !replaced.contains(&l.unversioned())),
    );
    let mut seen = HashSet::new();
    for library in libraries {
        if !allowed(&library.rules, os, arch) {
            continue;
        }
        if let Some(artifact) = library.artifact()?
            && seen.insert(artifact.path.clone())
        {
            classpath.push(artifact.path.clone());
            files.push(artifact);
        }
        if let Some(mut native) = library.native(os, arch)?
            && seen.insert(native.path.clone())
        {
            native.unpack = Some(Unpack {
                to: natives.clone(),
                exclude: library
                    .extract
                    .as_ref()
                    .map(|e| e.exclude.clone())
                    .unwrap_or_default(),
            });
            files.push(native);
        }
    }

    let client = vanilla
        .downloads
        .as_ref()
        .map(|d| &d.client)
        .ok_or_else(|| {
            error(format!(
                "Minecraft {} has no client to download",
                vanilla.id
            ))
        })?;
    let jar = rel(&format!("versions/{0}/{0}.jar", vanilla.id))?;
    classpath.push(jar.clone());
    files.push(RuntimeFile {
        path: jar,
        url: client.url.clone(),
        size: client.size,
        sha1: client.sha1.clone(),
        unpack: None,
        executable: false,
    });

    let index_ref = vanilla
        .asset_index
        .as_ref()
        .expect("checked before fetching it");
    files.push(RuntimeFile {
        path: rel(&format!("assets/indexes/{}.json", index_ref.id))?,
        url: index_ref.url.clone(),
        size: index_ref.size,
        sha1: index_ref.sha1.clone(),
        unpack: None,
        executable: false,
    });
    // Old versions read assets by name rather than by hash.
    let legacy = index.r#virtual || index.map_to_resources;
    let game_assets = match legacy {
        true => rel(&format!("assets/virtual/{}", index_ref.id))?,
        false => rel("assets")?,
    };
    for (name, object) in &index.objects {
        let hash = &object.hash;
        let prefix = hash
            .get(..2)
            .ok_or_else(|| error(format!("asset {name} has no hash")))?;
        let url = format!("{RESOURCES}/{prefix}/{hash}");
        let file = RuntimeFile {
            path: rel(&format!("assets/objects/{prefix}/{hash}"))?,
            url,
            size: Some(object.size),
            sha1: Some(hash.clone()),
            unpack: None,
            executable: false,
        };
        if legacy {
            let mut named = file.clone();
            named.path = rel(&format!("{game_assets}/{name}"))?;
            files.push(named);
        }
        files.push(file);
    }

    let (mut jvm_args, mut game_args) = vanilla.args(os, arch);
    let main_class = match &loader {
        Some(loader) => {
            let (jvm, game) = loader.args(os, arch);
            if loader.minecraft_arguments.is_some() {
                game_args = game;
            } else {
                game_args.extend(game);
            }
            jvm_args.extend(jvm);
            loader.main_class.clone().or(vanilla.main_class.clone())
        }
        None => vanilla.main_class.clone(),
    };
    let main_class = main_class.ok_or_else(|| error(format!("{id} doesn't say what to run")))?;

    let mut vars = BTreeMap::from([
        ("version_name".to_owned(), id.clone()),
        ("assets_index_name".to_owned(), index_ref.id.clone()),
        (
            "classpath_separator".to_owned(),
            match os {
                Os::Windows => ";",
                _ => ":",
            }
            .to_owned(),
        ),
    ]);
    if let Some(kind) = &vanilla.kind {
        vars.insert("version_type".to_owned(), kind.clone());
    }
    let dirs = BTreeMap::from([
        ("assets_root".to_owned(), rel("assets")?),
        ("game_assets".to_owned(), game_assets),
        ("library_directory".to_owned(), rel("libraries")?),
        ("natives_directory".to_owned(), natives),
    ]);
    let java = vanilla
        .java_version
        .as_ref()
        .map(|j| j.major_version)
        // from before versions said, all of which run on Java 8
        .or(Some(8));
    Ok(Setup {
        id,
        files,
        launch: Launch::Java(JavaLaunch {
            java,
            java_exe: None,
            main_class,
            classpath,
            jvm_args,
            game_args,
            vars,
            dirs,
        }),
    })
}

/// Whether `rules` (Mojang's, from a library or an argument) allow it on
/// `os`. Arguments for features (a demo, a custom window size, ...) are
/// left out, since none are turned on.
fn allowed(rules: &[Rule], os: Os, arch: &str) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        let os_matches = rule.os.as_ref().is_none_or(|r| {
            r.name.as_deref().is_none_or(|n| n == os_name(os))
                && r.arch.as_deref().is_none_or(|a| arch_matches(a, arch))
        });
        let features_match = rule.features.values().all(|on| !on);
        if os_matches && features_match {
            allowed = rule.action == Action::Allow;
        }
    }
    allowed
}

fn os_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Macos => "osx",
    }
}

/// Whether Mojang's name for an architecture is `arch`, Rust's.
fn arch_matches(mojang: &str, arch: &str) -> bool {
    match mojang {
        "x86" => arch == "x86",
        "x86_64" | "amd64" => arch == "x86_64",
        "arm64" | "aarch64" => arch == "aarch64",
        _ => false,
    }
}

/// `${arch}` in old natives' classifiers.
fn bits(arch: &str) -> &'static str {
    match arch {
        "x86" | "arm" => "32",
        _ => "64",
    }
}

fn rel(path: &str) -> Result<RelPath> {
    RelPath::new(path).map_err(|e| error(format!("{path}: {e}")))
}

fn error(message: String) -> Error {
    Error::Provisioner {
        kind: KIND.to_owned(),
        message,
    }
}

async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    let reqwest_error = |result| Error::Reqwest {
        url: url.to_owned(),
        result,
    };
    let bytes = patchwork_core::http_get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(reqwest_error)?
        .bytes()
        .await
        .map_err(reqwest_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Debug, Deserialize)]
struct Manifest {
    versions: Vec<ManifestVersion>,
}

#[derive(Debug, Deserialize)]
struct ManifestVersion {
    id: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct LoaderVersions {
    loader: LoaderVersion,
}

#[derive(Debug, Deserialize)]
struct LoaderVersion {
    version: String,
    #[serde(default)]
    stable: Option<bool>,
}

/// One of Mojang's version files, or a loader's profile in the same form.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionJson {
    id: String,
    #[serde(default)]
    inherits_from: Option<String>,
    #[serde(default)]
    main_class: Option<String>,
    #[serde(default)]
    arguments: Option<Arguments>,
    /// What versions before 1.13 have in place of `arguments`.
    #[serde(default)]
    minecraft_arguments: Option<String>,
    #[serde(default)]
    libraries: Vec<Library>,
    #[serde(default)]
    downloads: Option<Downloads>,
    #[serde(default)]
    asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    java_version: Option<JavaVersion>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

impl VersionJson {
    /// Its JVM and game arguments that apply on `os`.
    fn args(&self, os: Os, arch: &str) -> (Vec<String>, Vec<String>) {
        if let Some(args) = &self.arguments {
            let pick = |args: &[Argument]| {
                args.iter()
                    .flat_map(|a| match a {
                        Argument::Plain(s) => vec![s.clone()],
                        Argument::Ruled { rules, value } if allowed(rules, os, arch) => {
                            value.clone().into_vec()
                        }
                        Argument::Ruled { .. } => Vec::new(),
                    })
                    .collect()
            };
            return (pick(&args.jvm), pick(&args.game));
        }
        let Some(game) = &self.minecraft_arguments else {
            return (Vec::new(), Vec::new());
        };
        // what Mojang's launcher gave versions that didn't say
        let mut jvm = Vec::new();
        if os == Os::Macos {
            jvm.push("-XstartOnFirstThread".to_owned());
        }
        jvm.extend(
            [
                "-Djava.library.path=${natives_directory}",
                "-cp",
                "${classpath}",
            ]
            .map(String::from),
        );
        (jvm, game.split_whitespace().map(String::from).collect())
    }
}

#[derive(Debug, Default, Deserialize)]
struct Arguments {
    #[serde(default)]
    game: Vec<Argument>,
    #[serde(default)]
    jvm: Vec<Argument>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Argument {
    Plain(String),
    Ruled { rules: Vec<Rule>, value: OneOrMany },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Rule {
    action: Action,
    #[serde(default)]
    os: Option<OsRule>,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Allow,
    Disallow,
}

#[derive(Debug, Deserialize)]
struct OsRule {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Library {
    /// Its Maven coordinates, `group:artifact:version[:classifier]`.
    name: String,
    #[serde(default)]
    downloads: Option<LibraryDownloads>,
    /// The Maven repository it's in, for libraries without `downloads`
    /// (loaders' own).
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    rules: Vec<Rule>,
    /// The classifier of its natives for each OS, for versions before
    /// 1.19, whose natives are unpacked rather than put on the classpath.
    #[serde(default)]
    natives: BTreeMap<String, String>,
    #[serde(default)]
    extract: Option<Extract>,
}

impl Library {
    /// `group:artifact[:classifier]`, which a loader's library of the
    /// same name takes the place of whatever its version.
    fn unversioned(&self) -> String {
        let mut parts: Vec<&str> = self.name.split(':').collect();
        if parts.len() > 2 {
            parts.remove(2);
        }
        parts.join(":")
    }

    /// Its jar for the classpath, if it has one. Libraries that are only
    /// natives don't.
    fn artifact(&self) -> Result<Option<RuntimeFile>> {
        if let Some(downloads) = &self.downloads {
            return downloads
                .artifact
                .as_ref()
                .map(|a| self.file(a, None))
                .transpose();
        }
        if !self.natives.is_empty() {
            return Ok(None);
        }
        let path = maven_path(&self.name, None)
            .ok_or_else(|| error(format!("can't tell where library `{}` is", self.name)))?;
        let repo = self.url.as_deref().unwrap_or(LIBRARIES);
        let repo = repo.trim_end_matches('/');
        Ok(Some(RuntimeFile {
            url: format!("{repo}/{path}"),
            path: rel(&format!("libraries/{path}"))?,
            size: self.size,
            sha1: self.sha1.clone(),
            unpack: None,
            executable: false,
        }))
    }

    /// Its natives for `os`, for versions before 1.19.
    fn native(&self, os: Os, arch: &str) -> Result<Option<RuntimeFile>> {
        let Some(classifier) = self.natives.get(os_name(os)) else {
            return Ok(None);
        };
        let classifier = classifier.replace("${arch}", bits(arch));
        let artifact = self
            .downloads
            .as_ref()
            .and_then(|d| d.classifiers.get(&classifier));
        match artifact {
            Some(artifact) => self.file(artifact, Some(&classifier)).map(Some),
            None => Err(error(format!(
                "library `{}` has no {classifier} to download",
                self.name
            ))),
        }
    }

    fn file(&self, artifact: &Artifact, classifier: Option<&str>) -> Result<RuntimeFile> {
        let path = match &artifact.path {
            Some(path) => path.clone(),
            None => maven_path(&self.name, classifier)
                .ok_or_else(|| error(format!("can't tell where library `{}` is", self.name)))?,
        };
        Ok(RuntimeFile {
            path: rel(&format!("libraries/{path}"))?,
            url: artifact.url.clone(),
            size: artifact.size,
            sha1: artifact.sha1.clone(),
            unpack: None,
            executable: false,
        })
    }
}

/// Where the Maven artifact `coords` (`group:artifact:version`, with an
/// optional `:classifier` and `@extension`) is in a repository.
fn maven_path(coords: &str, classifier: Option<&str>) -> Option<String> {
    let (coords, ext) = coords.split_once('@').unwrap_or((coords, "jar"));
    let mut parts = coords.split(':');
    let (group, artifact, version) = (parts.next()?, parts.next()?, parts.next()?);
    let classifier = classifier.or(parts.next());
    if group.is_empty() || artifact.is_empty() || version.is_empty() || parts.next().is_some() {
        return None;
    }
    let file = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Some(format!(
        "{}/{artifact}/{version}/{file}",
        group.replace('.', "/")
    ))
}

#[derive(Debug, Deserialize)]
struct LibraryDownloads {
    #[serde(default)]
    artifact: Option<Artifact>,
    #[serde(default)]
    classifiers: BTreeMap<String, Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    #[serde(default)]
    path: Option<String>,
    url: String,
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Extract {
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Downloads {
    client: Artifact,
}

#[derive(Debug, Deserialize)]
struct AssetIndexRef {
    id: String,
    url: String,
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JavaVersion {
    major_version: u32,
    /// Which of Mojang's Java runtimes it runs on.
    #[serde(default)]
    component: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JavaRuntime {
    manifest: Artifact,
    version: JavaRuntimeVersion,
}

#[derive(Debug, Deserialize)]
struct JavaRuntimeVersion {
    name: String,
}

#[derive(Debug, Deserialize)]
struct JavaManifest {
    files: BTreeMap<String, JavaEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum JavaEntry {
    File {
        downloads: JavaDownloads,
        #[serde(default)]
        executable: bool,
    },
    Directory,
    Link,
}

#[derive(Debug, Deserialize)]
struct JavaDownloads {
    raw: Artifact,
}

#[derive(Debug, Deserialize)]
struct AssetIndex {
    objects: BTreeMap<String, AssetObject>,
    #[serde(default)]
    r#virtual: bool,
    #[serde(default)]
    map_to_resources: bool,
}

#[derive(Debug, Deserialize)]
struct AssetObject {
    hash: String,
    size: u64,
}

#[cfg(test)]
mod tests;
