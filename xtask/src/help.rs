use colored::Colorize;

pub(crate) fn print_help() {
    eprintln!(
        "\nUsage: {}
Tasks are run in the root of the repository.

Tasks:
all           Run all task before drafting a PR
build-aarch64 Build the project for aarch64
build-x64     Build the project for x86_64
check         Run cargo check
clippy        Run cargo clippy
coverage      Generate code coverage report
cspell        Print words that cspell does not recognize
deny          Run cargo deny
docs          Generate documentation
fmt           Run cargo fmt
help          Print this help message
setup         Install prerequisite tools
test          Run tests

Options:
Task specific cargo options can be passed after the task name, e.g.:
cargo xtask build-x64 --release
cargo xtask doc --open
",
        "cargo xtask <task> [options]".bright_green()
    );
}
