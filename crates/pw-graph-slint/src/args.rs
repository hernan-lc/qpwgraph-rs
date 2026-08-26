use pw_graph_i18n::I18n;

#[derive(Clone, Debug, Default)]
pub(crate) struct Args {
    pub(crate) minimized: bool,
    pub(crate) debug: bool,
    pub(crate) no_midi: bool,
    pub(crate) language: Option<String>,
    pub(crate) demo: bool,
}

pub(crate) fn parse_args() -> Args {
    let mut args = Args::default();
    let parser_i18n = I18n::from_language(&std::env::var("LANG").unwrap_or_default());
    let mut values = std::env::args().skip(1);

    while let Some(value) = values.next() {
        match value.as_str() {
            "-m" | "--minimized" => args.minimized = true,
            "-d" | "--debug" => args.debug = true,
            "-n" | "--no-midi" | "--no-alsa-midi" => args.no_midi = true,
            "--demo" => args.demo = true,
            "--lang" => args.language = values.next(),
            value if value.starts_with("--lang=") => {
                args.language = Some(value.trim_start_matches("--lang=").to_owned())
            }
            "-h" | "--help" => {
                println!(
                    "qpwgraph-rs\n\n{}\n  -m, --minimized       {}\n  -d, --debug           {}\n  -n, --no-midi         {}\n      --lang <LANG>     {}\n      --demo             {}",
                    parser_i18n.text("cli.options"),
                    parser_i18n.text("cli.minimized"),
                    parser_i18n.text("cli.debug"),
                    parser_i18n.text("cli.no_midi"),
                    parser_i18n.text("cli.lang"),
                    parser_i18n.text("cli.demo"),
                );
                std::process::exit(0);
            }
            unknown => eprintln!(
                "{}",
                parser_i18n.format("cli.unknown_option", &[("option", unknown.into())])
            ),
        }
    }

    args
}
