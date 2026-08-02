---
title: Jane Doe
role: Digital Lead
email: jane.doe@acme.example
client: "[[acme-corp]]"
client_domain: "acme.example"
---

# Jane Doe

Primary contact for Acme Corp. The bare `client` link resolves through the
Relation field's `target: clients` setting.

`client_domain` is a read-only Lookup. Because `client` is scalar here, the
computed value is also scalar. Change `clients/acme-corp.md#domain` to see it
update without editing this document.
