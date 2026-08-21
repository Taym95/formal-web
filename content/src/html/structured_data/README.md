# content/src/html/structured_data

Implements the HTML spec's "safe passing of structured data"
(<https://html.spec.whatwg.org/#safe-passing-of-structured-data>): the
structured serialization algorithms that turn JavaScript values into
IPC-serializable records and back.

The folder splits the spec section between the generic algorithms and the
per-platform-object parts of those algorithms, so a platform object's
transfer/`[Serializable]` handling does not grow the algorithm file:

- `safe_passing_of_structured_data.rs` — the generic algorithms:
  `StructuredSerializeInternal`, `StructuredDeserialize`,
  `StructuredSerializeWithTransfer`, `StructuredDeserializeWithTransfer`,
  the `structuredClone` API, plus the `Serializable` / `Transferable`
  traits and the `MemoryMap`.  No platform-object-specific code lives here:
  transferable platform objects plug in through the `Transferable` trait,
  and the per-interface transfer steps live in their own module.
- `messageport.rs` — the MessagePort-specific parts: recognizing a
  transferable MessagePort (the [[Detached]]-slot check of
  `StructuredSerializeWithTransfer` step 2.1), running its transfer steps
  and building its data holder (step 5.2), and rebuilding the port on the
  receiving side (`StructuredDeserializeWithTransfer` step 3.2).

The wire-format data (`SerializedRecord`, `TransferDataHolder`,
`PortTransferData`, `PortMessagePayload`, `PostMessageRequest`) lives in
`ipc_messages::safe_passing_of_structured_data` — it is the wire format and
must be defined in the crate both processes link.

The MessagePort platform object itself stays in
`content/src/html/messageport.rs` (it implements the `Transferable` trait
for `PortTransferData`); this folder only holds the parts of the
safe-passing algorithms that are specific to it.  Future transferable or
`[Serializable]` platform objects get their own module here the same way.

## Gotchas

See the "Structured clone" section of `content/src/html/README.md` for the
serialization pitfalls that apply to the generic algorithms (UTF-16 string
round-tripping, RegExp `[[OriginalSource]]`, Error `message` via
`[[GetOwnProperty]]`, enumerability filtering, wrapper objects, error
`cause`).
