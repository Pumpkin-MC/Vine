# Vine

A fast, lightweight Minecraft proxy for **Java & Bedrock Edition** written in Rust, powered by [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) with WebAssembly plugin support.

## Supported Servers

- **Pumpkin** (`vine` / `velocity` forwarding)
- **Paper / Purpur / Folia** (`velocity` / `bungeecord` forwarding)
- **Spigot / Bukkit** (`bungeecord` forwarding)
- **Vanilla** (offline-mode, `none` forwarding)
- **Bedrock Edition** (NetherNet)

## Usage

```bash
cargo run --release
```

On first run, `vine.toml` is generated automatically. Pass a custom config path if needed:

```bash
cargo run --release -- path/to/vine.toml
```

## License

[GPL-3.0](LICENSE)
