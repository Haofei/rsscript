# Missing Capability Binding

The RSS code still calls the native facade `S3.put_object`, but `rsspkg.toml`
does not bind that symbol to an external capability.

Reviewer question: does absence of metadata make an external native call look
safe?

Expected result: the demo reports an unknown capability binding and treats the
package as a failed review input under `deny_unknown`.
