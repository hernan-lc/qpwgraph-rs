use pw_graph_i18n::I18n;

#[derive(Clone, Debug, Default)]
pub(crate) struct Args {
    minimized: bool,
    debug: bool,
    no_alsa_midi: bool,
    language: Option<String>,
    demo: bool,
}
