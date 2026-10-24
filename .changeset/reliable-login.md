---
"@agent-wechat/agent-server": patch
"@agent-wechat/shared": patch
---

Keep login execution on the server when viewers disconnect, expose cheap versioned auth snapshots and replayable login phases, and require verified identity and readable core stores before success. Bound helper lifetime and serialize data preparation. Unknown client builds validate raw and known-layout candidates using page HMACs and schema reads; core login preparation skips optional media setup and writes private temporary files.
