# Per-built-Agent identity witness

The framework provides AgentIdentity as a small, pure read-only witness of
one actual AgentBuilder build. Agent.identity() captures the witness;
AgentIdentity.matches() checks that built instance. matches_backend() also
checks the actual Agent's immutable backend field against a retained backend.

Every build creates a fresh private marker. Sharing the same model, state,
backend, registry, tools, name or thread cannot supply the marker to another
builder. Cloning or moving a built Agent retains its identity. There is no
caller constructor, identity setter, raw pointer/hash value, Debug or Serialize
interface. A witness retains only the marker, not state or backend resources.

The identity does not grant read/write, task, source, model, native or execution
permission. The private app capture owner additionally checks its actual
selected frontend context, state/tool bindings, retained root, read policy and
generation. No model/tool/middleware/native/default/lifecycle behavior or
framework dependencies change. Session.agent keeps its existing Agent type.

Pure framework tests exercise clone/move/thread stability, fresh builds sharing
all supplied handles, and the actual backend comparison without dispatching
a model or tool. Full local/hosted/native/source-tree and independent-main gates
still apply before delivery integration.
