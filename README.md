# opencc600

Firmware libre pour le compteur vélo **GEOID CC600** (ESP32-S3, carte Magene C606 sans eMMC ni tactile).

Statut : définition produit et reverse terminés, code à venir. Les drivers matériels viendront de
[c606-oss](https://github.com/HighwayStar/c606-oss) (GPL-3.0).

## Docs
- [docs/PRODUCT.md](docs/PRODUCT.md) : décisions, contraintes, questions produit, plan technique, workflow flash / retour arrière.
- [docs/NOTE.md](docs/NOTE.md) : notes de reverse (matériel, eFuses, partitions, pinout lu par JTAG, essai de c606-oss).
- [docs/pins_live.txt](docs/pins_live.txt) : sortie brute de `tools/read_pins.py` sur le firmware officiel.

Dans les notes, les chemins `dump/…` et `c606-oss/…` désignent des fichiers locaux qui ne sont pas dans ce dépôt :
le dump de la flash (firmware propriétaire), et un clone de c606-oss.

## Outils
- `tools/read_pins.py` : lit le routage des broches d'un ESP32-S3 en marche via l'USB-JTAG intégré
  (OpenOCD Espressif), en lecture seule.

## Licence
GPL-3.0-or-later, voir [LICENSE](LICENSE).
