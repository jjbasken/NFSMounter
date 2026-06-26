# NFS Mount Manager

A minimal GTK4/libadwaita GUI for mounting and unmounting NFS shares on Linux.

## Features

- Mount and unmount NFS shares with a polished GNOME-style interface
- Privilege elevation via `pkexec` — no need to run the app as root
- Optional mount options field (e.g. `vers=4,rw`)
- Live mount status indicator (reads `/proc/mounts`)
- History of up to 10 recent mounts, persisted across sessions

## Dependencies

- GTK 4
- libadwaita ≥ 1.6
- `pkexec` (part of PolicyKit / `polkit`)
- An NFS client (`nfs-utils` or equivalent)

## Building

```sh
cargo build --release
```

The binary will be at `target/release/nfs-mount`.

## Running

```sh
./target/release/nfs-mount
```

When you click **Mount** or **Unmount**, `pkexec` will prompt for your password to run the privileged `mount`/`umount` command.

## History

Mount history is saved to `$XDG_CONFIG_HOME/nfs-mount/history` (typically `~/.config/nfs-mount/history`).

## License

GNU General Public License v3.0 — see [LICENSE](LICENSE).
