project(
  'obscura',
  version: '0.2.0',
  meson_version: '>= 1.4',
  license: 'GPL-3.0-or-later',
)

i18n = import('i18n')
gnome = import('gnome')

app_id = 'io.github.porthole_dev.Obscura'

dependency('gtk4', version: '>= 4.20')
dependency('libadwaita-1', version: '>= 1.8')
dependency('libcamera', version: '>= 0.7')
dependency('gstreamer-1.0', version: '>= 1.24')
dependency('gstreamer-app-1.0', version: '>= 1.24')
dependency('gstreamer-tag-1.0', version: '>= 1.24')

cargo = find_program('cargo')
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)
glib_compile_schemas = find_program('glib-compile-schemas')

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')
podir = meson.project_source_root() / 'po'

subdir('data')
subdir('po')
subdir('src')

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
