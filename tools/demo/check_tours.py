#!/usr/bin/env -S uv run -q --python python3 --with playwright==1.55.0 python3
"""Replay the guided visits through their visible UI, on the real WASM firmware.

Usage: tools/demo/check_tours.py http://127.0.0.1:8765/?v=local
   or: tools/demo/check_tours.py --site target/demo/site --version local
CHROMIUM can select a browser executable. Reports go to target/demo/tour-check/.
"""
import argparse
from contextlib import contextmanager
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
from threading import Thread

from playwright.sync_api import sync_playwright


@contextmanager
def address(args):
    if not args.site:
        yield args.url
        return
    class QuietHandler(SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), partial(QuietHandler, directory=args.site))
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/?v={args.version}'
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def check(url, output):
    output.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(executable_path=os.environ.get('CHROMIUM') or shutil.which('google-chrome'))
        context = browser.new_context(viewport={'width': 1280, 'height': 1100})
        page = context.new_page()
        page.set_default_timeout(90_000)
        errors, visited = [], []
        page.on('pageerror', lambda e: errors.append(str(e)))

        def log_since(text, since=0):
            page.wait_for_function('([text, since]) => document.querySelector("#console").textContent.slice(since).includes(text)', arg=[text, since])

        def command(line, reply):
            since = len(page.locator('#console').inner_text())
            # Setup/read-back use the existing USB form without opening a different tab over the guide.
            page.locator('#line').evaluate('(el, line) => el.value = line', line)
            page.locator('#cmd').evaluate('(form) => form.requestSubmit()')
            log_since(reply, since)

        def saved_flash():
            return page.evaluate('''async () => {
                const db = await new Promise((resolve, reject) => {
                    const r = indexedDB.open('opencc600-demo', 1);
                    r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error);
                });
                const rows = await new Promise((resolve, reject) => {
                    const r = db.transaction('flash').objectStore('flash').getAll();
                    r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error);
                });
                return await Promise.all(rows.map(async (row) => {
                    const hash = await crypto.subtle.digest('SHA-256', await row.data.arrayBuffer());
                    return Array.from(new Uint8Array(hash));
                }));
            }''')

        def restored(button='#tour-finish'):
            since = len(page.locator('#console').inner_text())
            page.locator(button).click()
            page.wait_for_function('() => document.querySelector("#tours").dataset.state === "idle"')
            log_since('splash: done', since)
            command('get backlight', 'usb: backlight=37')
            command('get hr', 'usb: hr=none')

        try:
            page.goto(url, wait_until='domcontentloaded')
            page.screenshot(path=str(output / 'first-impression.png'))
            page.locator('[data-tour="power"]').wait_for(state='visible')
            page.wait_for_timeout(700)  # capture the end of the notes' entrance, not an animation frame
            page.screenshot(path=str(output / 'suggestions.png'))
            # Give free exploration a real, persistent setting before entering a visit.
            page.locator('.key.tl').focus(); page.keyboard.down('Enter')
            page.wait_for_timeout(700); page.keyboard.up('Enter')
            log_since('splash: done')
            command('set backlight 37', 'settings: saved')
            for name in ('ride', 'sensors', 'power'):
                if name == 'power':
                    page.set_viewport_size({'width': 390, 'height': 844})
                page.locator(f'[data-tour="{name}"]').click()
                before = None
                while True:
                    page.wait_for_function('() => ["active", "complete", "error"].includes(document.querySelector("#tours").dataset.state)')
                    state = page.locator('#tours').get_attribute('data-state')
                    assert state != 'error', page.locator('#tour-instruction').inner_text()
                    if state == 'complete':
                        break
                    step = page.locator('#tours').get_attribute('data-step')
                    print(f'{name}: {step}', flush=True)
                    assert page.get_by_role('button', name='Suivant', exact=True).count() == 0
                    if step == 'home':
                        page.wait_for_function('() => document.querySelector("#tours").dataset.state !== "active" || document.querySelector("#tours").dataset.step !== "home"')
                        continue
                    if before is None:
                        command('get backlight', 'usb: backlight=50')
                        before = saved_flash()
                        assert before, 'free exploration was not saved before the visit'
                    # The arrow must point at the real current control, including after responsive layout.
                    page.wait_for_function('''() => {
                        const p = document.querySelector('#tour-pointer'), t = document.querySelector('.tour-target');
                        if (!p || p.hasAttribute('hidden') || !t || document.querySelector(p.dataset.target) !== t) return false;
                        const r = t.getBoundingClientRect(), x = Number(p.dataset.x), y = Number(p.dataset.y);
                        return Math.abs(y - r.top - r.height / 2) < 3
                            && Math.min(Math.abs(x - r.left), Math.abs(x - r.right)) <= 16;
                    }''')
                    if page.viewport_size['width'] > 980:
                        assert page.locator('#device').evaluate('(el) => Math.abs(el.getBoundingClientRect().left + el.offsetWidth / 2 - innerWidth / 2) < 1'), 'counter lost its central position'
                    if step in ('play', 'enable'):
                        assert page.locator('#tour-tools').is_visible()
                        assert page.locator('#tour-tools .tour-target').count() == 1
                        page.wait_for_timeout(150)
                        page.screenshot(path=str(output / f'{name}-{step}.png'))
                    if step != 'home':
                        assert page.locator('#tour-finish').is_hidden()
                        if name == 'ride' and step == 'start':
                            # The guide must also recognize the visitor pressing the actual key.
                            page.locator('.key.br').focus(); page.keyboard.down('Enter')
                            page.wait_for_timeout(100); page.keyboard.up('Enter')
                        else:
                            page.locator('#tour-perform').click()
                    # A correct action must change the instruction without clicking another button.
                    page.wait_for_function('(step) => document.querySelector("#tours").dataset.state !== "active" || document.querySelector("#tours").dataset.step !== step', arg=step)
                    assert page.locator('#tours').get_attribute('data-state') != 'error', page.locator('#tour-instruction').inner_text()
                    if step in ('page', 'pair', 'charge'):
                        page.screenshot(path=str(output / f'{name}-{step}.png'))
                completed = json.loads(page.locator('#tours').get_attribute('data-completed'))
                assert len(completed) == int(page.locator('#tour-bar').get_attribute('max'))
                visited.extend(f'{name}:{step}' for step in completed)
                assert not errors, errors
                log = page.locator('#console').inner_text()
                assert 'panicked' not in log and 'PANIC' not in log, 'firmware panic'
                if name == 'ride':
                    assert ': started' in log and ': saved,' in log
                elif name == 'sensors':
                    assert 'sensors: HeartRate = ant:120:6001:1' in log and 'screen Ride Cardio' in log
                else:
                    assert 'screen Charging' in log and 'charging screen: switched on' in log
                (output / f'{name}.log').write_text(log)
                assert page.locator('#tour-pointer').is_hidden()
                assert page.locator('#tour-tools').is_hidden()
                assert page.locator('#panel-route #play').count() == 1
                assert page.locator('#panel-sensors #sensor-cards').count() == 1
                page.locator('#tours').screenshot(path=str(output / f'{name}.png'))
                assert saved_flash() == before, 'a visit overwrote the free-exploration flash'
                restored()
            # Abort during a visit, then reload during another one: both must restore the free data.
            page.locator('[data-tour="ride"]').click()
            page.wait_for_function('() => document.querySelector("#tours").dataset.state === "active" && document.querySelector("#tours").dataset.step !== "home"')
            restored('#tour-quit')
            page.locator('[data-tour="power"]').click()
            page.wait_for_function('() => document.querySelector("#tours").dataset.state === "active" && document.querySelector("#tours").dataset.step !== "home"')
            page.reload(wait_until='domcontentloaded')
            page.locator('[data-tour="power"]').wait_for()
            assert page.locator('#tours').get_attribute('data-state') == 'idle'
            page.locator('.key.tl').focus(); page.keyboard.down('Enter')
            page.wait_for_timeout(700); page.keyboard.up('Enter')
            log_since('splash: done')
            command('get backlight', 'usb: backlight=37')
            command('get hr', 'usb: hr=none')
            assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'mobile page overflows horizontally'
            assert not errors, errors
            (output / 'report.json').write_text(json.dumps({'steps': visited, 'errors': errors, 'freeStoragePreserved': True, 'abortAndReload': True, 'mobileWidth': 390}, indent=2))
            print(f'{len(visited)} steps verified on the real firmware; no browser errors', flush=True)
        except Exception:
            (output / 'failure.log').write_text(page.locator('#console').inner_text())
            page.screenshot(path=str(output / 'failure.png'), full_page=True)
            raise
        finally:
            browser.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('url', nargs='?', default='http://127.0.0.1:8765/?v=local')
    parser.add_argument('--site')
    parser.add_argument('--version', default='local')
    parser.add_argument('--output', type=Path, default=Path('target/demo/tour-check'))
    args = parser.parse_args()
    with address(args) as url:
        check(url, args.output)
