import re

with open("src/ssh.rs", "r") as f:
    text = f.read()

text = re.sub(
    r"fn check_server_key<'a>\([^\}]+\}?",
    r"fn check_server_key(self: &mut Self, _server_public_key: &ssh_key::PublicKey) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {\n        async { Ok(true) }\n    }",
    text
)

with open("src/ssh.rs", "w") as f:
    f.write(text)
