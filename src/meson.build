# Like Alpine's Rust packages: embed the dependency tree when cargo-auditable
# is available. CARGO_NET_OFFLINE=true in the environment makes this --frozen.
cargo_auditable = find_program('cargo-auditable', required: false)
cargo_build = cargo_auditable.found() ? [cargo, 'auditable', 'build'] : [cargo, 'build']

cargo_env = environment()
cargo_env.set('LOCALEDIR', localedir)

target_dir = meson.project_build_root() / 'target'
# A cross build names its target through CARGO_BUILD_TARGET (cargo's own
# variable), and cargo then writes to target/<triple>/release rather than
# target/release.
cargo_target = run_command('sh', '-c', 'printf %s "${CARGO_BUILD_TARGET:-}"',
  check: true).stdout().strip()
release_dir = cargo_target == '' ? target_dir / 'release' : target_dir / cargo_target / 'release'

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
    'cp', release_dir / meson.project_name(), '@OUTPUT@',
  ],
)
