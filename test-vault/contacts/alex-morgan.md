---
title: Alex Morgan
role: Fractional Advisor
email: alex.morgan@example.test
client:
  - "[[clients/acme-corp]]"
  - "[[clients/globex]]"
  - "[[clients/initech]]"
client_domain: ["acme.example","globex.example","initech.example"]
---

# Alex Morgan

Alex works across three accounts. The `client_domain` Lookup preserves this
relation's list cardinality and order, producing an ordered list of all three
current Client domains without flattening or hand-copying them.
