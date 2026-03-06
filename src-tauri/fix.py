import re

with open("src/ssh.rs", "r") as f:
    content = f.read()

# Fix check_server_key signature
content = re.sub(
    r"async fn check_server_key\(\s*&mut self,\s*_server_public_key: &russh::keys::key::PublicKey,\s*\) -> Result<bool, Self::Error> \{.*?\n\s*\}",
    "fn check_server_key<'a>(&'a mut self, _server_public_key: &'a ssh_key::PublicKey) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send + 'a {\n        async { Ok(true) }\n    }",
    content,
    flags=re.DOTALL
)

# Remove auth_keyboard_interactive from ClientHandler trait
content = re.sub(
    r"\n\s*async fn auth_keyboard_interactive[^\}]+?\n\s*\}\n",
    "\n",
    content,
    flags=re.DOTALL
)

# Fix prompts error
content = content.replace("client::Prompt<'_>", "client::Prompt")

# Fix p.prompt.iter mapping for trait bounds
content = content.replace("p.prompt.as_ref()", "p.prompt.as_str()")

# Fix missing connection_timeout field
content = re.sub(r"\s*connection_timeout: Some\(std::time::Duration::from_secs\(10\)\),", "", content)

# Fix authenticate_keyboard_interactive function name
content = content.replace("session.authenticate_keyboard_interactive(user.clone(), \"\").await", "session.authenticate_keyboard_interactive_start(user.clone(), None).await")

# Fix match branches for KeyboardInteractiveAuthResponse
content = content.replace("Ok(true) => {", "Ok(russh::client::KeyboardInteractiveAuthResponse::Success) => {")
content = content.replace("Ok(false) => {", "Ok(russh::client::KeyboardInteractiveAuthResponse::Failure) => {")

with open("src/ssh.rs", "w") as f:
    f.write(content)
