//! CLI entrypoint for `tau provider` subcommands.

const HELP_TEXT: &str = "\
Usage: tau provider <provider> <subcommand>

Builtin providers:
  chatgpt             ChatGPT / Codex OAuth provider
  chat-completions    OpenAI-compatible Chat Completions provider

Examples:
  tau provider chatgpt login
  tau provider chat-completions add";

/// Run the provider CLI with the given provider-specific subcommand arguments.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str).unwrap_or("help") {
        "chatgpt" => tau_ext_provider_openai::run_provider_cli(&args[1..]),
        "chat-completions" => tau_ext_provider_chat_completions::run_provider_cli(&args[1..]),
        "help" | "--help" | "-h" => {
            println!("{HELP_TEXT}");
            Ok(())
        }
        other => {
            eprintln!("unknown provider: {other}");
            eprintln!("{HELP_TEXT}");
            Err(format!("unknown provider: {other}").into())
        }
    }
}

#[cfg(test)]
mod tests;
