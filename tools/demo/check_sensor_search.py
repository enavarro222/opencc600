#!/usr/bin/env -S uv run -q --python python3 --with playwright==1.55.0 python3
"""Search and pair all demo sensors using only the counter's buttons.

Usage: tools/demo/check_sensor_search.py http://127.0.0.1:8765/?v=local
Requires a staged/running demo with ANT+ support. CHROMIUM can select the browser executable.
"""
import os
import shutil
import sys

from playwright.sync_api import sync_playwright


def check(url: str) -> None:
    with sync_playwright() as pw:
        browser = pw.chromium.launch(executable_path=os.environ.get('CHROMIUM') or shutil.which('google-chrome'))
        page = browser.new_page(viewport={'width': 1280, 'height': 960})
        page.set_default_timeout(30_000)
        errors = []
        assets = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: assets.append(request.url))

        def wait_log(text: str, since: int = 0) -> None:
            page.wait_for_function(
                '([text, since]) => document.querySelector("#console").textContent.slice(since).includes(text)',
                arg=[text, since],
            )

        def key(which: str) -> None:
            page.locator(f'.key.{which}').click()
            page.wait_for_timeout(400)

        try:
            page.goto(url, wait_until='domcontentloaded')
            page.locator('.key.tl').focus()
            page.keyboard.down('Space')
            page.wait_for_timeout(650)
            page.keyboard.up('Space')
            wait_log('splash: done')
            page.locator('#tab-sensors').click()
            assert 'Simulation ANT+ disponible' in page.locator('#sensor-availability').inner_text()
            for sensor in ('hr', 'csc', 'power'):
                page.locator(f'#sensor-{sensor}-enabled').check()
            key('bl')  # Home -> Menu (Capteurs is the first entry on a clean device)
            key('br')  # Menu -> Capteurs
            for i, (role, kind, number) in enumerate((('HeartRate', 120, 6001), ('SpeedCadence', 121, 6002), ('Power', 11, 6003))):
                since = len(page.locator('#console').inner_text())
                key('br')  # Chercher
                wait_log(f'ant: found ant:{kind}:{number}:1', since)
                page.wait_for_timeout(400)  # wait for the found row to be drawn and made selectable
                key('br')  # Choisir
                wait_log(f'sensors: {role} = ant:{kind}:{number}:1', since)
                wait_log(f'ant: channel {kind} ({number & 255}-1) state 3', since)
                print(f'{role}: found, selected with the counter buttons, connected')
                if i < 2:
                    key('bl')
            assert not errors, errors
            for suffix in ('worker.js', 'esp32sim.wasm', 'cc600-flash.bin', 'cc600.elf'):
                assert any(f'{suffix}?build=' in url for url in assets), f'unversioned asset: {suffix}'
            print('Browser errors: none; emulator and firmware assets carry their build identifier')
        finally:
            browser.close()


if __name__ == '__main__':
    check(sys.argv[1] if len(sys.argv) > 1 else 'http://127.0.0.1:8765/')
