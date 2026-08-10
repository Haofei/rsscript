#!/usr/bin/env bash
set -euo pipefail

# Require an ADR whenever a change touches a compatibility/security contract.
# The caller supplies the merge base (or previous push SHA). Keeping the rule in
# a small checked-in script makes local and CI enforcement identical.
base="${1:-HEAD^}"
if ! git rev-parse --verify --quiet "${base}^{commit}" >/dev/null; then
  echo "ADR gate: base revision ${base} is unavailable; skipping empty-history comparison."
  exit 0
fi

contract_changed=false
adr_changed=false
while IFS= read -r path; do
  [[ -z "${path}" ]] && continue
  case "${path}" in
    crates/rsscript-syntax/*|crates/rsscript-semantics/*|crates/rsscript-abi-model/*|crates/rsscript-mir/*|crates/rsscript-bytecode/*|crates/rsscript-provider-api/*|crates/rsscript-sdk/*)
      contract_changed=true
      ;;
  esac
  case "${path}" in
    docs/architecture/adr/[0-9][0-9][0-9][0-9]-*.md)
      adr_changed=true
      ;;
  esac
done < <(git diff --name-only "${base}"...HEAD)

if [[ "${contract_changed}" == true && "${adr_changed}" != true ]]; then
  cat >&2 <<'EOF'
ADR gate: this change touches an ABI, MIR, bytecode, Provider, or stable SDK
contract. Add or update docs/architecture/adr/NNNN-short-title.md using the
checked-in template. If the change is implementation-only, document why the
public contract is unaffected in that ADR.
EOF
  exit 1
fi
