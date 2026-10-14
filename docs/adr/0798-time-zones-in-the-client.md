# 0798. Time zones in the client

## Status

Accepted

## Context

A daily quiet time names a zone of the IANA time zone database, and the server holds it while the clock of that zone shows a time inside the window (ADR 0412). A client must offer the zone of the system as the first choice and a list of zones to choose from, and it applies the rule of the quiet time to what it holds by the clock of the zone (ADR 0792). The client core holds no time zone data: its function takes the minute of the day from its caller. macOS and Linux have the database as files; Windows has none.

## Decision

- The window crate has one module for zones: every name of the database in the order of the alphabet, a search of the names, the name of the zone of the system, and the minute of the day that the clock of a named zone shows at an instant.
- The names and the rules come from the crate `chrono-tz`, which carries the database in the binary. The list is the same on the three systems and does not depend on a file of the system. The crate uses `chrono`, which the window has for the times of messages.
- The name of the zone of the system comes from the crate `iana-time-zone`, which `chrono` uses. A system that states no name, or a name that the build does not hold, gives `UTC`.
- A name that the build does not hold gives no clock, and so no quiet time, as an unknown zone does on the server.
- The search ignores case and takes a space for the underscore of a name. Names whose last part starts with the typed text come first. A search returns at most 50 names and states that more exist.
- The server decides with its own database whether it accepts a name. A name of the list that the server refuses is stated as a refusal of the request.

## Consequences

- The database of a build is the one of its release of `chrono-tz`. A zone whose rules changed afterwards has the old clock in the client until a new build, while the server uses its own database. A test sets a quiet time in several zones against the real server and checks that the server holds a notice back at the minute that the client computed.
- The binary grows by the tables of the database.
- The settings of the window send the zone of the system with a daily quiet time, and the window applies a quiet time by the clock of the zone that the settings name. A control that chooses another zone is not built.
