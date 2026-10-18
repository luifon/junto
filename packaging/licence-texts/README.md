# Standard licence texts

The licence file of a client package (ADR 0801, `packaging/licences.py`) gives a crate whose published package holds no licence file, or whose licence files do not hold the text of a licence that its manifest requires, the standard text of each such licence. The Apache License 2.0 is the file `LICENSE` at the root of the repository. The files here are the texts of the SPDX License List, release 3.27.0 (`https://github.com/spdx/license-list-data/tree/v3.27.0/text`), unchanged; each file is named by its SPDX identifier.

When the assembly of a package stops because a crate has no text of its licence and its licence has no text here, add the file `<identifier>.txt` from the same release of the list, and the phrases of the licence to `LICENCE_PHRASES` in `licences.py` when it is not there; a test requires that each file here is identified as its own licence.
