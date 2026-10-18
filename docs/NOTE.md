# GEOID CC600 — notes reverse / custom firmware

## Matériel identifié (2026-10-19)

- USB : `303a:1001` Espressif USB JTAG/serial debug unit → `/dev/ttyACM0`
  (USB-Serial/JTAG natif de l'ESP32-S3, pas de puce UART externe)
- SoC : **ESP32-S3 (QFN56) rev v0.2**, dual core 240 MHz, Wi-Fi + BLE 5
- PSRAM : 2 Mo embarquée (AP_3v3)
- Flash : **16 Mo**, quad, 3.3 V (manuf 0x85 = Puya, device 0x2018)
- MAC : (retirée)
- Console série : silencieuse au boot (rien reçu en 6 s à 115200)

## Sécurité (eFuses) — tout est ouvert

| eFuse | Valeur |
|---|---|
| SPI_BOOT_CRYPT_CNT (flash encryption) | Disable |
| SECURE_BOOT_EN | False |
| KEY_PURPOSE_0..5 | USER (aucune clé) |
| DIS_DOWNLOAD_MODE / DIS_USB_SERIAL_JTAG_DOWNLOAD_MODE | False |
| DIS_USB_JTAG / DIS_PAD_JTAG / SOFT_DIS_JTAG | False / False / 0 |

→ lecture/écriture flash libre via esptool, debug JTAG possible via le même câble USB (OpenOCD).
Résumé complet : `dump/efuse_summary.txt`.

## Dump flash

`dump/flash_full_16MB.bin` — sha256 `adf4c11d8fdba077d6b486498b5485460519c59601454e62c5c40fdb36c58350`
**Sauvegarde de référence : à garder hors du repo, en plusieurs exemplaires.**
Restauration : `esptool -p /dev/ttyACM0 write-flash 0 flash_full_16MB.bin`

### Table de partitions (0x8000)

| Nom | Type | Offset | Taille | Remarque |
|---|---|---|---|---|
| nvs | data/nvs | 0x009000 | 16K | réglages, appairages BLE probablement |
| otadata | data/ota | 0x00d000 | 8K | seq 1 / seq 2 → **ota_1 actif** |
| phy_init | data/phy | 0x00f000 | 4K | |
| coredump | data/coredump | 0x010000 | 64K | contient un ELF de crash (exploitable pour les symboles/stack) |
| ota_0 | app | 0x020000 | 4224K | `Venus_V1.313-dirty`, 21 août 2024, IDF `e3fda883-dirty` |
| ota_1 | app | 0x440000 | 4224K | `506_SE_V1.407-dirty`, 5 mars 2026, IDF `17977806-dirty` (actif) |
| storage | data/0x83 (littlefs) | 0x860000 | 7800K | `/littlefs/FITS/FIT/*.fit` = sorties enregistrées, + `ModuleDataTest` |

Partitions extraites dans `dump/parts/`. Les deux apps sont des images ESP-IDF standard
(checksum + SHA256 valides, project name "Project", secure version 0).

## Le CC600 est un Magene C606 rebadgé

- Geoid = marque sœur de Magene (Qingdao). Le firmware CC600 contient les chaînes `CC600` et `MageneC606`
  côte à côte (`dump/parts/ota_1.strings.txt` l.542-544).
- FCC : CC600 = probablement [2A2L5-345](https://fccid.io/2A2L5-345) (Geoid, 2024-09-25), "Change in ID" de
  [2ALZG-309](https://fccid.io/2ALZG-309) (Magene, 2024-08-26) → **photos internes du PCB dispo** :
  https://fccid.io/2ALZG-309/Internal-Photos/Internal-Photos-7601615 (fccid.io bloque curl, ouvrir dans un navigateur)
- CC700 Pro = [2A2L5-390](https://fccid.io/2A2L5-390), proche du Magene C606 Pro.
- Même config mémoire que le C606 : 16 Mo flash + 2 Mo PSRAM quad.

### Projet existant : c606-oss
Upstream : https://github.com/HighwayStar/c606-oss (GPL-3.0, cloné dans `c606-oss/`, dernier commit 2026-10-01).
Fork moins à jour : https://github.com/an0nfunc/c606-oss.
Doc clé : `c606-oss/docs/HARDWARE.md` (pinout C606, init ST7789, protocole nRF, section CC700 Pro).
Recovery : maintenir les 3 boutons = hard reset de l'ESP32.

### CC600 ≠ C606 exact : pas d'eMMC
- Partitions C606 : ota_0/ota_1 de 0x73A000 chacun, **pas de partition storage** (tout sur l'eMMC `/sdcard`).
- Partitions CC600 : ota_0/ota_1 de 0x420000 + **littlefs 7,8 Mo sur la flash**.
- Firmware CC600 : 0 chaîne `/sdcard`, 89 chaînes `/littlefs` (FITS, TMP, ModuleDataTest…), marques
  `BRAND_OVERSEA_GEOID` / `BRAND_*_MAGENE` → **CC600 = C606 allégé sans eMMC** (cohérent avec l'absence de cartes).
- Conséquences pour c606-oss : le montage eMMC échouera (sans dégâts, `format_if_mount_failed = false`) →
  il faudra un `BOARD=cc600` qui stocke en littlefs ; l'app doit tenir dans 0x420000 (4,1 Mo) ;
  pins à confirmer par JTAG (écran 16 bits C606 ou 8 bits C606 Pro ?).

### Pinout lu en direct par JTAG (2026-10-19)
Script : `tools/read_pins.py` (OpenOCD Espressif, `board/esp32s3-builtin.cfg`, halt ~1 s, lecture seule,
attache sans reset). Sortie : `dump/pins_live.txt`, log brut `dump/jtag_pins_raw.log`.
Prérequis : règle udev `60-openocd.rules` d'OpenOCD dans `/etc/udev/rules.d/`.

| Fonction | CC600 (live) | c606-oss `BOARD=c606` |
|---|---|---|
| LCD i80 **16 bits** D0..D15 | 4,38,5,37,6,36,7,35,8,34,9,33,10,47,11,48 | identique |
| LCD DC / WR / CS / RD | 40 / 3 / 2 / 39 (out=1) | identique |
| Rétroéclairage | 45 (LEDC) | 45 |
| nRF UART2 TX / RX | 42 / 41, 115200 baud | 42 / 41, 115200 |
| GPS UART0 TX / RX | 1 / 0, ~921600 baud | 1 / 0, 921600 |
| I2C touch SDA 21 / SCL 12 | **non configurés** → pas de tactile | FT6336 / CST328 |
| SDMMC 13-18 | **non configurés** → pas d'eMMC | eMMC 4 Go |
| GPIO43/44 | U0TXD/U0RXD par défaut (pas forcés à 1) | — |

**Conclusion : CC600 = carte C606 sans eMMC ni tactile.** Écran, nRF, GPS identiques →
`BOARD=c606` devrait afficher et lire boutons/GPS tel quel. À adapter pour un `BOARD=cc600` :
désactiver touch + eMMC, stockage FIT/config sur la partition littlefs `storage` (0x860000), app ≤ 0x420000.

### Premier essai c606-oss `BOARD=c606` (2026-10-19)
- Build : Docker `espressif/idf:v5.4.2` (le script fourni est pour podman) :
  `docker run --rm -u $(id -u):$(id -g) -e HOME=/tmp -v "$PWD:/project" -w /project -e IDF_TARGET=esp32s3 espressif/idf:v5.4.2 idf.py -B build -DBOARD=c606 build`
  → `build/c606_oss.bin` = 0xF5E20 (~1 Mo), tient dans ota_0 (0x420000).
- Flash : `tools/flash_poc.py -p /dev/ttyACM0 --board c606` (depuis `dump/flash_poc_backups/`,
  backups identiques à `dump/parts/ota_0.bin`). Écrit ota_0, efface otadata.
- Attention : `--dry-run` laisse l'ESP en bootloader ROM → écran éteint ; `esptool --after hard-reset read-mac` relance.
- Résultat au boot (`dump/first_boot_c606oss.log`, **contient la position GPS**) :
  écran/LVGL OK, nRF UART2 OK (handshake), GPS AG3352Q_V3.1.0 @ 921600, fix en ~6 s,
  touche 2 → ride démarre ; **eMMC absente → pas d'enregistrement** ("riding without recording").
- **Retour firmware officiel** : `esptool -p /dev/ttyACM0 write-flash 0xd000 dump/parts/otadata.bin`
  (otadata d'origine → boot sur ota_1 V1.407). **Fait le 2026-10-19, boot sur 0x440000 vérifié.**
  ota_0 contient encore c606-oss (inactif).
- Verdict : drivers OK, mais l'app c606-oss ne convient pas → on garde c606-oss comme BSP,
  firmware à nous par-dessus. Suite : `PRODUCT.md`.

### Projet existant : c606-oss (README du fork)
https://github.com/an0nfunc/c606-oss (fork de HighwayStar/c606-oss) — firmware open source ESP-IDF + LVGL 9
pour Magene C606 / C606 Pro / C706 **et Geoid CC700 Pro** (testé sur un vrai appareil le 2026-09-23).
Hardware du C606 d'après le README (donc très probablement le CC600) :
- écran 240x320 **ST7789 en i80 16 bits**, tactile FT6336 (I2C)
- **coprocesseur nRF** : boutons, alimentation, radios ANT+/capteurs. Lien UART avec trames + CRC16,
  handshake "power-on" vendor obligatoire. c606-oss ne remplace que l'app ESP32, pas le nRF.
- GNSS Airoha AG3352Q sur UART0 (NMEA)
- eMMC 4 Go en FAT sur `/sdcard` — **à vérifier sur le CC600** : nos FIT sont dans la littlefs de la flash
  (le firmware contient pourtant "hello emmc!")
- flash : `tools/flash_poc.py` sauvegarde ota_0+otadata, écrit uniquement ota_0, `--restore` pour revenir
- pièges : lecture flash avec le stub qui échoue sur un secteur (relire avec `--no-stub`) ;
  ouvrir le port avec DTR/RTS désactivés reset le S3

## Plan d'attaque

### Phase 0 — Sécuriser (fait)
- [x] eFuses lus, aucune protection
- [x] dump complet 16 Mo + partitions extraites
- [ ] 2e dump et comparaison des sha256 (valider que la lecture est fiable)
- [ ] copier le dump hors de la machine

### Phase 1 — Hardware (ouvrir le boîtier) → probablement inutile
Remplacée en priorité par : photos FCC internes 2ALZG-309 + pinout c606-oss + lecture des pins par JTAG
sur le firmware officiel (méthode utilisée par c606-oss sur le CC700 Pro).
Le firmware ne dit pas tout : il faut savoir ce qui est câblé à quels GPIO.
- photos recto/verso du PCB, références des puces
- écran (contrôleur, interface SPI/8080/RGB, rétroéclairage), GNSS (modèle, UART), boutons, buzzer/LED,
  capteurs (baro, accéléro), gestion batterie/charge, éventuelle puce ANT+ séparée
- repérer GPIO0/BOOT et EN/RESET (récupération si le firmware custom casse l'USB)

### Phase 2 — Analyse statique du firmware officiel
- Ghidra + extension ESP32 (ou plugin Xtensa), charger `ota_1.bin` segment par segment
  (adresses de chargement via `esptool image-info`), ou convertir en ELF (`esp32_image_parser`)
- retrouver les symboles des fonctions IDF/ROM (ROM ELF Espressif, signatures FLIRT/BSim
  depuis une build IDF de la même version) pour isoler le code applicatif
- objectif : **table des GPIO** (init SPI écran, UART GNSS, I2C capteurs, boutons) et séquence d'init de l'écran
- le coredump ELF donne des adresses/stack réelles : bon point d'entrée
- outils type REA envisageables à cette étape si ça aide à naviguer

### Phase 3 — Analyse dynamique (optionnelle)
- OpenOCD (`board/esp32s3-builtin.cfg`) + gdb sur le firmware officiel en marche :
  breakpoints sur `gpio_config`, `spi_bus_initialize`, `uart_driver_install`, `i2c_param_config`
  pour lire directement les pins utilisés
- dump du contenu NVS (`nvs_partition_tool` d'IDF) et de la littlefs (`littlefs-python`) pour comprendre les formats

### Phase 4 — Premier firmware custom minimal
- ESP-IDF (ou Arduino/PlatformIO) ciblant ESP32-S3, flash 16 Mo, PSRAM
- écrire **uniquement dans ota_0** (garder ota_1 officiel intact) puis basculer otadata ;
  retour arrière = réécrire otadata ou le dump complet
- étapes : log USB → GPIO/boutons → écran (LVGL) → GNSS → BLE (capteurs FTMS / puissance / cardio)
- lien avec le projet home_trainer_game : compteur qui parle FTMS au home trainer

### Garde-fous
- ne **jamais** toucher aux eFuses en écriture (irréversible)
- ne pas écraser bootloader (0x0) / table de partitions (0x8000) au début
- garder le dump de référence avant chaque écriture
