# Firmware custom CC600 — définition produit et plan

Contexte matériel et reverse : voir `NOTE.md`. Ce fichier sert de point de départ à la session de code
(autre workspace VS Code).

## Décisions prises (2026-10-19)
- **Base** : c606-oss (https://github.com/HighwayStar/c606-oss) utilisé comme **couche matérielle**,
  pas comme application. Son UI/app ne nous convient pas.
- **Licence** : GPL-3.0-or-later (ok de réutiliser le code c606-oss tel quel).
- **Architecture** : extraire les drivers dans un composant ESP-IDF `cc600_bsp`, et écrire notre propre
  firmware par-dessus.
- **Cible matérielle** : CC600 = carte Magene C606 **sans eMMC ni tactile** (pinout vérifié par JTAG, voir NOTE.md).

## Ce qu'on reprend de c606-oss (couche BSP)
| Brique | Fichiers c606-oss | État sur CC600 |
|---|---|---|
| Écran ST7789 i80 16 bits + LVGL 9 | `main/ui_port.c`, `board_c606.h`, `board.h` | OK |
| Lien nRF (UART2, trames CRC16, handshake, boutons, batterie, temp/pression, ANT+) | `main/nrf*.c` | OK (ANT+ non testé) |
| GPS Airoha AG3352Q (UART0 921600, NMEA, PAIR) | `main/gps.c` | OK, fix ~6 s |
| Écriture FIT | `main/fit*.c` | à rebrancher sur littlefs |
| Doc de référence | `docs/HARDWARE.md` | — |

À retirer/adapter pour le CC600 : `sdcard.c` / `usb_msc.c` (eMMC), touch (FT6336/CST328), cartes Mapsforge.

## Contraintes matérielles
- 240x320 couleur, **3 boutons, pas de tactile** → ergonomie 100 % boutons.
- ESP32-S3, PSRAM 2 Mo quad, app ≤ **0x420000 (4,1 Mo)** par slot OTA.
- Stockage : partition littlefs `storage` 7,8 Mo (0x860000) — **contient les sorties officielles** :
  écrire dans un sous-dossier dédié, ou sauvegarder avant.
- BLE + Wi-Fi sur l'ESP32 ; ANT+ via le nRF (non reprogrammable).
- Batterie 1800 mAh : autonomie = critère (Wi-Fi et rétroéclairage coûteux).
- RTC du nRF non écrit par c606-oss → heure fausse avant le premier fix GPS.

## Questions produit (à remplir)
1. **Usage** : route, home trainer, les deux ? pour qui ?
2. **Manques du firmware officiel** qui justifient le projet :
3. **Cœur à ne jamais casser** : enregistrement FIT fiable (crash, batterie vide), affichage, capteurs.
4. **Différenciation** (idées) : pilotage home trainer FTMS, intégration home_trainer_game, Zwift Click /
   télécommandes BLE, écrans entièrement configurables, séances structurées…
5. **Synchro des sorties** : Wi-Fi (Strava / serveur perso), BLE vers app, USB MSC sur la littlefs ?
6. **Configuration** : menus 3 boutons, page web en Wi-Fi, app ?

### MVP (à définir)
- [ ] …

### Hors périmètre volontaire
- cartes / navigation turn-by-turn (pas d'eMMC)
- …

## Plan technique
1. Nouveau repo (GPL-3.0), ESP-IDF 5.4, cible esp32s3, partition table identique au vendor
   (ota_0 / ota_1 0x420000, storage littlefs conservée).
2. Composant `cc600_bsp` : écran, nRF, GPS, boutons, batterie (copie/refacto depuis c606-oss, crédits conservés).
3. Stockage littlefs (`esp_littlefs`) monté sur `storage`, sous-dossier dédié.
4. App : UI LVGL + logique de sortie + FIT.
5. Puis synchro / config / différenciation selon le MVP.

## Workflow flash / retour arrière (validé le 2026-10-19)
- Build : Docker `espressif/idf:v5.4.2` (voir NOTE.md).
- Flash : uniquement `ota_0` + effacement `otadata` (`c606-oss/tools/flash_poc.py` fait ça proprement).
- **Retour firmware officiel** : `esptool -p /dev/ttyACM0 write-flash 0xd000 dump/parts/otadata.bin`
  → boot sur `ota_1` V1.407 (vérifié).
- Dump complet de référence : `dump/flash_full_16MB.bin` (sha256 dans NOTE.md).
- Recovery : 3 boutons maintenus = hard reset ; ROM download mode accessible via USB à tout moment.
- Outils à réinstaller dans la nouvelle session (ceux de cette session étaient temporaires) :
  `uv tool install esptool`, OpenOCD Espressif (release `openocd-esp32` linux-amd64, règle udev
  `60-openocd.rules` déjà installée), script de lecture du pinout : `tools/read_pins.py`.
- **Jamais** d'écriture eFuse, ni bootloader (0x0) / table de partitions (0x8000).
