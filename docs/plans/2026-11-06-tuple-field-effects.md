# Tuple field provenance

Investigate the remaining false send-buffer escapes in the authenticated client.
Tuple construction/destructuring currently merges every field into one memory
set. A JSON object pair's string key can therefore inherit its value's pointer
graph. Preserve index-labelled tuple fields, with conservative dynamic-index and
raw-memory fallbacks. Test literal projection, destructuring and nested patterns,
including a genuinely escaping selected pointer. Verify the live client memory
gate before claiming the diagnosis resolves the leak. Update ownership docs and
BUGS alongside the change, preserving previous comments.
