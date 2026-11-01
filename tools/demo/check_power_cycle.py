#!/usr/bin/env -S uv run -q --python python3 --with playwright==1.55.0 python3
"""Check auto-off, the power key and USB wake with the real Musette iframe (about 90 seconds).

Usage: tools/demo/check_power_cycle.py http://127.0.0.1:8765/?v=local
The staged site must include Musette. No settings or flash from a visitor's browser are used.
"""
import os
import re
import shutil
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright


def check(url: str) -> None:
    with sync_playwright() as pw:
        browser = pw.chromium.launch(executable_path=os.environ.get('CHROMIUM') or shutil.which('google-chrome'))
        page = browser.new_page(viewport={'width': 1400, 'height': 1100})
        page.set_default_timeout(30_000)
        app = page.frame_locator('#musette')
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))

        def log_count(text: str, count: int = 1, timeout: int = 30_000) -> None:
            page.wait_for_function(
                '([text, count]) => document.querySelector("#console").textContent.split(text).length - 1 >= count',
                arg=[text, count], timeout=timeout,
            )

        def key(which: str) -> None:
            page.locator(f'.key.{which}').click()
            page.wait_for_timeout(400)

        def hold() -> None:
            page.locator('.key.tl').focus()
            page.keyboard.down('Space')
            page.wait_for_timeout(850)
            page.keyboard.up('Space')

        def save_count() -> int:
            return page.locator('#console').inner_text().count('ble: app changed settings')

        def bips(on: bool) -> None:
            before = save_count()
            app.get_by_label(re.compile('^Bips')).set_checked(on)
            log_count('ble: app changed settings', before + 1)
            app.get_by_text('Enregistré sur le compteur.', exact=True).wait_for()

        try:
            page.goto(url, wait_until='domcontentloaded')
            hold()
            log_count('splash: done')
            page.locator('#tab-sensors').click()
            page.locator('#sensor-hr-enabled').check()  # broadcasts even while the counter is off
            page.locator('#musette-open-cta').click()
            app.get_by_role('button', name='Compteur', exact=True).click()
            app.get_by_role('button', name='Connecter un compteur', exact=True).click()
            app.get_by_role('button', name='Connecter le compteur de la démo', exact=True).click()
            app.get_by_role('button', name='Déconnecter', exact=True).wait_for()
            app.get_by_text('Énergie', exact=True).click()
            before = save_count()
            app.get_by_label(re.compile('^Extinction automatique')).fill('1')
            log_count('ble: app changed settings', before + 1)
            app.get_by_text('Enregistré sur le compteur.', exact=True).wait_for()
            app.get_by_role('button', name='‹ Retour', exact=True).click()
            app.get_by_text('Sons', exact=True).click()
            key('br')  # start a stationary ride, which triggers auto-off after one minute
            log_count(': started')
            print('Musette connected; waiting for automatic power off with a sensor broadcasting', flush=True)
            page.wait_for_function('() => document.querySelector("#device").classList.contains("powered-off")', timeout=90_000)
            assert not page.locator('#usb-plug').get_attribute('aria-label').startswith('Câble USB-C branché')
            page.wait_for_timeout(1200)
            hold()
            log_count('opencc600: booted', 2)
            log_count('splash: done', 2)
            log_count('ble: app connected', 2)
            bips(False)  # same iframe, same app session, real write + read-back after waking
            print('Auto-off -> power key -> Musette write/read: OK', flush=True)
            # The recovered ride's pause menu: Sensors, then Power off (no stored route in a fresh browser).
            key('tl'); key('bl'); key('br')
            page.wait_for_function('() => document.querySelector("#device").classList.contains("powered-off")')
            page.locator('#usb-plug').click()
            log_count('opencc600: booted', 3)
            log_count('splash: done', 3)
            log_count('ble: app connected', 3)
            hold()  # leave the charging screen
            bips(True)
            assert not errors, errors
            assert 'storage: littlefs error' not in page.locator('#console').inner_text()
            print('Power off -> USB wake -> Musette write/read: OK; browser errors: none', flush=True)
        except Exception:
            Path('/tmp/opencc600-power-cycle.log').write_text(page.locator('#console').inner_text())
            page.screenshot(path='/tmp/opencc600-power-cycle-failure.png', full_page=True)
            raise
        finally:
            browser.close()


if __name__ == '__main__':
    check(sys.argv[1] if len(sys.argv) > 1 else 'http://127.0.0.1:8765/?v=local')
