use clap::ValueEnum;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum ServerMode {
    #[default]
    Code,
    Tools,
}
