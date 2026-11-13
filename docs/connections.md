# Local state and configured connections

Craftsman keeps sessions, memories, skills and gadgets on the local machine.
It has no platform login, skill marketplace, inbox polling or workspace service.
The ACP bridge serves its configured client transport without starting an inbox
worker. Retired `platform` and login fields remain inert compatibility data;
empty platform fields are omitted when configuration is saved.

Model connections live in `providers.NAME` or the legacy direct `base_url` and
`api_key` fields. A named protocol uses its documented standard API endpoint
when `base_url` is omitted. An explicitly supplied legacy OpenAI key uses the
standard OpenAI endpoint. A model name alone does not initialize a connection.
Credentials may reference environment variables explicitly in configuration.

`/model` reads the configured catalog locally, groups models by provider and
opens a separate effort or thinking-budget picker after a model is chosen.
The final choice saves and applies the model and its setting together. Closing
either menu retains the active configuration. Each model keeps its own settings.
`/model --refresh` explicitly discovers models from the configured connections;
the bounded lookup remains cancellable. `/effort` changes the current model's
setting directly. See [provider models](provider-models.md).

MCP servers run only from `mcp_servers` entries. Each entry specifies its HTTP
endpoint or stdio command, arguments and headers/environment. See
[the MCP example](../examples/config/mcp-servers.json). `/skills` lists local
`SKILL.md` files and has no remote install, update or delete action. Previously
installed skill directories remain local files and are retained.

Web search needs a configured provider: a Brave `search_api_key`, or an explicit
`"search_provider": "duckduckgo"` choice for keyless search. The restricted
`service-agent` uses these same settings. Without them, search returns a setup
message. Web fetch uses a requested URL under the existing permission checks;
gadget HTTP uses declared host capabilities and existing approvals.

Voice transcription uses local Whisper models. A missing model is downloaded
only when `voice.allow_model_download` is explicitly `true`; existing cached
models do not need that permission. Model names, download revisions, hashes and
native audio/shutdown limits are unchanged. This policy concerns configured
services and explicit operations, not an OS network sandbox.
