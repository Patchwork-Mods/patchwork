# DRG integration assets

`integration/` is copied unchanged from [mint](https://github.com/trumank/mint)
(`assets/integration`, commit `57aa964e7789f132481813a8f4b5781f924eccd1`), by
Truman Kilen and contributors, under the MIT licence in `LICENSE-mint`. The
deployer packs these blueprints into `mods_P.pak`; the game's patched
blueprints spawn `MI_SpawnMods`, which starts each mod's `InitSpacerig` and
`InitCave`.
