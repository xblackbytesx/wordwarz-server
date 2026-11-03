# Word lists

Built into `nl.fst` and `en.fst` by `make lexicons` (`scripts/lexicons.sh`).
The `.fst` files are checked in: the server embeds them and the app ships
them as assets. Normalization: lowercase only (no proper nouns or
abbreviations), letters only (no hyphens, apostrophes, digits or spaces),
diacritics folded (`ideeën` -> `ideeen`), 2 to 19 letters.

| File | Source | Pin | Licence |
|---|---|---|---|
| `nl.fst` | OpenTaal Dutch word list, `wordlist.txt` | commit `b250510dda431785f962019167d1415198ff3905`, sha256 `12e5fb5e3c73840b583b30016926d1f63a75e9bf1652a3a6634b2ba7c49ad7be` | Revised BSD and/or CC BY 3.0 (we use BSD-3); `LICENSE-opentaal.txt` must travel with the file |
| `en.fst` | ENABLE (Enhanced North American Benchmark Lexicon), `norvig.com/ngrams/enable1.txt` | sha256 `61ba1392a5b6199dd161fae7b483cd262fb8a88c0d49bd2c9ecb6753e755697f` | Public domain |

Counts at these pins: 336,593 Dutch words (1.4 MiB), 172,541 English (490 KiB).

Attribution shown in the app's About screen and on the website:
"Dutch word list: OpenTaal (opentaal.org), BSD licence. English word list:
ENABLE, public domain."

Not usable: Collins Scrabble Words, NWL/TWL (licensed). ENABLE predates
some newer two-letter words (`qi`, `za` are not in it).
