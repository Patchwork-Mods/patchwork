use super::*;

fn vanilla() -> VersionJson {
    serde_json::from_value(serde_json::json!({
        "id": "1.21.1",
        "type": "release",
        "mainClass": "net.minecraft.client.main.Main",
        "javaVersion": { "majorVersion": 21 },
        "assetIndex": { "id": "17", "url": "https://example.com/17.json", "sha1": "aaa", "size": 10 },
        "downloads": { "client": { "url": "https://example.com/client.jar", "sha1": "ccc", "size": 3 } },
        "arguments": {
            "game": [
                "--version", "${version_name}",
                { "rules": [{ "action": "allow", "features": { "is_demo_user": true } }], "value": "--demo" }
            ],
            "jvm": [
                { "rules": [{ "action": "allow", "os": { "name": "osx" } }], "value": ["-XstartOnFirstThread"] },
                "-Djava.library.path=${natives_directory}",
                "-cp", "${classpath}"
            ]
        },
        "libraries": [
            {
                "name": "org.ow2.asm:asm:9.6",
                "downloads": { "artifact": {
                    "path": "org/ow2/asm/asm/9.6/asm-9.6.jar",
                    "url": "https://libraries.minecraft.net/org/ow2/asm/asm/9.6/asm-9.6.jar",
                    "sha1": "old", "size": 1
                } }
            },
            {
                "name": "org.lwjgl:lwjgl:3.3.3:natives-linux",
                "downloads": { "artifact": {
                    "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar",
                    "url": "https://libraries.minecraft.net/lwjgl-natives-linux.jar"
                } },
                "rules": [{ "action": "allow", "os": { "name": "linux" } }]
            },
            {
                "name": "org.lwjgl:lwjgl:3.3.3:natives-windows",
                "downloads": { "artifact": {
                    "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar",
                    "url": "https://libraries.minecraft.net/lwjgl-natives-windows.jar"
                } },
                "rules": [{ "action": "allow", "os": { "name": "windows" } }]
            }
        ]
    }))
    .unwrap()
}

fn fabric() -> VersionJson {
    serde_json::from_value(serde_json::json!({
        "id": "fabric-loader-0.16.5-1.21.1",
        "inheritsFrom": "1.21.1",
        "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "arguments": { "game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "] },
        "libraries": [
            { "name": "org.ow2.asm:asm:9.7.1", "url": "https://maven.fabricmc.net/", "sha1": "new", "size": 2 },
            { "name": "net.fabricmc:fabric-loader:0.16.5", "url": "https://maven.fabricmc.net/" }
        ]
    }))
    .unwrap()
}

fn index() -> AssetIndex {
    serde_json::from_value(serde_json::json!({
        "objects": { "minecraft/sounds/a.ogg": { "hash": "abcdef", "size": 5 } }
    }))
    .unwrap()
}

fn java(setup: &Setup) -> &JavaLaunch {
    let Launch::Java(java) = &setup.launch;
    java
}

fn paths(setup: &Setup) -> Vec<&str> {
    setup.files.iter().map(|f| f.path.as_str()).collect()
}

#[test]
fn vanilla_has_its_libraries_client_and_assets() {
    let setup = assemble(Os::Linux, "x86_64", vanilla(), None, &index()).unwrap();
    assert_eq!(setup.id, "1.21.1");
    assert_eq!(
        paths(&setup),
        [
            "libraries/org/ow2/asm/asm/9.6/asm-9.6.jar",
            "libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar",
            "versions/1.21.1/1.21.1.jar",
            "assets/indexes/17.json",
            "assets/objects/ab/abcdef",
        ]
    );
    let asset = setup.files.last().unwrap();
    assert_eq!(
        asset.url,
        "https://resources.download.minecraft.net/ab/abcdef"
    );
    assert_eq!(asset.sha1.as_deref(), Some("abcdef"));

    let java = java(&setup);
    assert_eq!(java.java, Some(21));
    assert_eq!(java.main_class, "net.minecraft.client.main.Main");
    assert_eq!(
        java.classpath.last().unwrap().as_str(),
        "versions/1.21.1/1.21.1.jar"
    );
    // no demo, and no macOS-only arguments on Linux
    assert_eq!(java.game_args, ["--version", "${version_name}"]);
    assert_eq!(
        java.jvm_args,
        [
            "-Djava.library.path=${natives_directory}",
            "-cp",
            "${classpath}"
        ]
    );
    assert_eq!(java.vars["version_type"], "release");
    assert_eq!(java.dirs["natives_directory"].as_str(), "natives/1.21.1");
}

#[test]
fn rules_pick_the_os() {
    let setup = assemble(Os::Macos, "aarch64", vanilla(), None, &index()).unwrap();
    assert!(!paths(&setup).iter().any(|p| p.contains("natives")));
    assert_eq!(java(&setup).jvm_args[0], "-XstartOnFirstThread");
    let setup = assemble(Os::Windows, "x86_64", vanilla(), None, &index()).unwrap();
    assert!(paths(&setup).iter().any(|p| p.contains("natives-windows")));
    assert_eq!(java(&setup).vars["classpath_separator"], ";");
}

#[test]
fn a_loader_goes_on_top_and_its_libraries_win() {
    let setup = assemble(Os::Linux, "x86_64", vanilla(), Some(fabric()), &index()).unwrap();
    assert_eq!(setup.id, "fabric-loader-0.16.5-1.21.1");
    let java = java(&setup);
    assert_eq!(
        java.main_class,
        "net.fabricmc.loader.impl.launch.knot.KnotClient"
    );
    let classpath: Vec<&str> = java.classpath.iter().map(RelPath::as_str).collect();
    assert_eq!(
        classpath,
        [
            "libraries/org/ow2/asm/asm/9.7.1/asm-9.7.1.jar",
            "libraries/net/fabricmc/fabric-loader/0.16.5/fabric-loader-0.16.5.jar",
            "libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar",
            "versions/1.21.1/1.21.1.jar",
        ]
    );
    let asm = &setup.files[0];
    assert_eq!(
        asm.url,
        "https://maven.fabricmc.net/org/ow2/asm/asm/9.7.1/asm-9.7.1.jar"
    );
    assert_eq!(asm.sha1.as_deref(), Some("new"));
    assert_eq!(
        java.jvm_args.last().unwrap(),
        "-DFabricMcEmu= net.minecraft.client.main.Main "
    );
    // the vanilla version's still what's in the version type and assets
    assert_eq!(java.vars["assets_index_name"], "17");
}

#[test]
fn a_loader_for_another_version_is_refused() {
    let mut fabric = fabric();
    fabric.inherits_from = Some("1.20.1".to_owned());
    let err = assemble(Os::Linux, "x86_64", vanilla(), Some(fabric), &index()).unwrap_err();
    assert!(err.to_string().contains("1.20.1"), "{err}");
}

#[test]
fn old_versions_unpack_their_natives_and_name_their_assets() {
    let old: VersionJson = serde_json::from_value(serde_json::json!({
        "id": "1.12.2",
        "mainClass": "net.minecraft.client.main.Main",
        "minecraftArguments": "--username ${auth_player_name} --version ${version_name}",
        "assetIndex": { "id": "1.12", "url": "https://example.com/1.12.json" },
        "downloads": { "client": { "url": "https://example.com/client.jar" } },
        "libraries": [{
            "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
            "natives": { "linux": "natives-linux", "windows": "natives-windows-${arch}" },
            "extract": { "exclude": ["META-INF/"] },
            "downloads": { "classifiers": {
                "natives-linux": { "path": "lwjgl-platform-natives-linux.jar", "url": "https://example.com/linux.jar" },
                "natives-windows-64": { "path": "lwjgl-platform-natives-windows-64.jar", "url": "https://example.com/win64.jar" }
            } }
        }]
    }))
    .unwrap();
    let index: AssetIndex = serde_json::from_value(serde_json::json!({
        "virtual": true,
        "objects": { "sounds/a.ogg": { "hash": "abcdef", "size": 5 } }
    }))
    .unwrap();
    let setup = assemble(Os::Windows, "x86_64", old, None, &index).unwrap();
    let native = &setup.files[0];
    assert_eq!(
        native.path.as_str(),
        "libraries/lwjgl-platform-natives-windows-64.jar"
    );
    let unpack = native.unpack.as_ref().unwrap();
    assert_eq!(unpack.to.as_str(), "natives/1.12.2");
    assert_eq!(unpack.exclude, ["META-INF/"]);
    assert!(paths(&setup).contains(&"assets/virtual/1.12/sounds/a.ogg"));

    let java = java(&setup);
    // natives aren't on the classpath, only the game
    assert_eq!(java.classpath.len(), 1);
    assert_eq!(java.java, Some(8));
    assert_eq!(
        java.jvm_args,
        [
            "-Djava.library.path=${natives_directory}",
            "-cp",
            "${classpath}"
        ]
    );
    assert_eq!(java.game_args[1], "${auth_player_name}");
    assert_eq!(java.dirs["game_assets"].as_str(), "assets/virtual/1.12");
}

#[test]
fn maven_coordinates_become_paths() {
    assert_eq!(
        maven_path("net.fabricmc:intermediary:1.21.1", None).unwrap(),
        "net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar"
    );
    assert_eq!(
        maven_path("a.b:c:1:natives-linux@zip", None).unwrap(),
        "a/b/c/1/c-1-natives-linux.zip"
    );
    assert!(maven_path("a.b:c", None).is_none());
}

#[test]
fn the_newest_stable_loader_is_picked() {
    let versions: Vec<LoaderVersions> = serde_json::from_value(serde_json::json!([
        { "loader": { "version": "0.17.0", "stable": false } },
        { "loader": { "version": "0.16.5", "stable": true } }
    ]))
    .unwrap();
    assert_eq!(pick_loader(&versions).unwrap(), "0.16.5");
    // Quilt doesn't say, but marks betas
    let versions: Vec<LoaderVersions> = serde_json::from_value(serde_json::json!([
        { "loader": { "version": "0.27.0-beta.1" } },
        { "loader": { "version": "0.26.4" } }
    ]))
    .unwrap();
    assert_eq!(pick_loader(&versions).unwrap(), "0.26.4");
}

#[test]
fn a_library_listed_for_its_jar_and_its_natives_has_both() {
    let version: VersionJson = serde_json::from_value(serde_json::json!({
        "id": "1.14.4",
        "mainClass": "net.minecraft.client.main.Main",
        "assetIndex": { "id": "1.14", "url": "https://example.com/1.14.json" },
        "downloads": { "client": { "url": "https://example.com/client.jar" } },
        "libraries": [
            {
                "name": "org.lwjgl:lwjgl:3.2.2",
                "downloads": { "artifact": { "path": "lwjgl.jar", "url": "https://example.com/lwjgl.jar" } }
            },
            {
                "name": "org.lwjgl:lwjgl:3.2.2",
                "natives": { "linux": "natives-linux" },
                "downloads": {
                    "artifact": { "path": "lwjgl.jar", "url": "https://example.com/lwjgl.jar" },
                    "classifiers": { "natives-linux": { "path": "lwjgl-natives-linux.jar", "url": "https://example.com/natives.jar" } }
                }
            }
        ]
    }))
    .unwrap();
    let setup = assemble(Os::Linux, "x86_64", version, None, &index()).unwrap();
    let libraries: Vec<&str> = paths(&setup)
        .into_iter()
        .filter(|p| p.starts_with("libraries/"))
        .collect();
    assert_eq!(
        libraries,
        ["libraries/lwjgl.jar", "libraries/lwjgl-natives-linux.jar"]
    );
    assert_eq!(java(&setup).classpath.len(), 2);
}

#[test]
fn java_runtimes_are_their_files_less_links_with_java_picked_out() {
    let manifest: JavaManifest = serde_json::from_value(serde_json::json!({
        "files": {
            "bin": { "type": "directory" },
            "bin/java": { "type": "file", "executable": true, "downloads": {
                "raw": { "url": "https://example.com/java", "sha1": "jjj", "size": 7 },
                "lzma": { "url": "https://example.com/java.lzma", "sha1": "zzz", "size": 3 }
            } },
            "lib/modules": { "type": "file", "downloads": {
                "raw": { "url": "https://example.com/modules", "sha1": "mmm", "size": 9 }
            } },
            "legal/java.se/LICENSE": { "type": "link", "target": "../java.base/LICENSE" }
        }
    }))
    .unwrap();
    let (files, exe) = java_files(&manifest, "java/delta/21.0.7", Os::Linux).unwrap();
    assert_eq!(exe.as_str(), "java/delta/21.0.7/bin/java");
    let found: Vec<(&str, &str, bool)> = files
        .iter()
        .map(|f| (f.path.as_str(), f.url.as_str(), f.executable))
        .collect();
    assert_eq!(
        found,
        [
            (
                "java/delta/21.0.7/bin/java",
                "https://example.com/java",
                true
            ),
            (
                "java/delta/21.0.7/lib/modules",
                "https://example.com/modules",
                false
            ),
        ]
    );

    // a runtime without the Java this system runs isn't any use
    assert!(java_files(&manifest, "java/delta/21.0.7", Os::Windows).is_err());
    assert_eq!(java_platform(Os::Linux, "aarch64"), None);
    assert_eq!(java_platform(Os::Macos, "aarch64"), Some("mac-os-arm64"));
}
