desktop_file = i18n.merge_file(
  input: app_id + '.desktop.in',
  output: app_id + '.desktop',
  type: 'desktop',
  po_dir: podir,
  install: true,
  install_dir: datadir / 'applications',
)
if desktop_file_validate.found()
  test('validate-desktop', desktop_file_validate, args: [desktop_file.full_path()], depends: desktop_file)
endif

metainfo_file = i18n.merge_file(
  input: app_id + '.metainfo.xml.in',
  output: app_id + '.metainfo.xml',
  po_dir: podir,
  install: true,
  install_dir: datadir / 'metainfo',
)
if appstreamcli.found()
  test('validate-metainfo', appstreamcli, args: ['validate', '--no-net', '--explain', metainfo_file.full_path()], depends: metainfo_file)
endif

install_data(app_id + '.gschema.xml', install_dir: datadir / 'glib-2.0' / 'schemas')
test('validate-gschema', glib_compile_schemas, args: ['--strict', '--dry-run', meson.current_source_dir()])

install_subdir('icons/hicolor', install_dir: datadir / 'icons')
