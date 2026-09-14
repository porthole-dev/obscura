# Like Alpine's Rust packages: embed the dependency tree when cargo-auditable
# is available. CARGO_NET_OFFLINE=true in the environment makes this --frozen.
cargo_auditable = find_program('cargo-auditable', required: false)
cargo_build = cargo_auditable.found() ? [cargo, 'auditable', 'build'] : [cargo, 'build']

cargo_env = environment()
cargo_env.set('LOCALEDIR', localedir)

target_dir = meson.project_build_root() / 'target'

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  env: cargo_env,
  command: [
    cargo_build, '--release', '--locked',
    '--manifest-path', meson.project_source_root() / 'Cargo.toml',
    '--target-dir', target_dir,
    '&&',
    'cp', target_dir / 'release' / meson.project_name(), '@OUTPUT@',
  ],
)
