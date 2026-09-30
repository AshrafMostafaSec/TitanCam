#!/usr/bin/env python3
"""Local desktop controller; never stores pairing credentials or records camera/audio."""
import fcntl
import ipaddress
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time
import gi

gi.require_version('Gtk', '4.0')
from gi.repository import GLib, Gtk

STATE = Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local/state')) / 'titancam'

class Application(Gtk.Application):
    def __init__(self):
        super().__init__(application_id='com.ashrafmostafasec.titancam.receiver')
        self.process = None
        self.generation = 0
        self.started = 0
        self.connect('activate', self.activate)
        self.connect('shutdown', lambda *_: self.stop())

    def activate(self, *_):
        if hasattr(self, 'window'):
            self.window.present()
            return
        STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(STATE, 0o700)
        self.lock = os.open(STATE / 'desktop.lock', os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            self.quit()
            return
        self.window = Gtk.ApplicationWindow(application=self, title='TitanCam — Linux')
        self.window.set_default_size(650, 700)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        for side in ('top', 'bottom', 'start', 'end'):
            getattr(box, 'set_margin_' + side)(22)
        self.window.set_child(box)
        title = Gtk.Label(label='TitanCam · iPhone → Linux', xalign=0)
        title.add_css_class('title-1')
        box.append(title)
        self.status = Gtk.Label(label='جاهز لاختبار الكاميرا والصوت', xalign=0, wrap=True)
        box.append(self.status)
        self.mode = Gtk.DropDown.new_from_strings(['USB — الكابل', 'Wi-Fi — الشبكة المحلية'])
        self.profile = Gtk.DropDown.new_from_strings(['Saver — 720p30', 'Balanced — 1080p60', 'Maximum — 4K60 تجريبي'])
        box.append(self.mode)
        box.append(self.profile)
        self.usb = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self.usb.append(Gtk.Label(label='افتح تطبيق الآيفون واضغط Enable USB connection.\nأدخل القيم المعروضة في التطبيق؛ الرمز صالح لدقيقتين.', xalign=0, wrap=True))
        self.pin = Gtk.Entry(placeholder_text='Certificate pin — بصمة الهاتف')
        self.token = Gtk.Entry(placeholder_text='One-time token — رمز الاقتران')
        self.token.set_visibility(False)
        self.port = Gtk.SpinButton.new_with_range(1024, 65533, 1)
        self.port.set_value(43052)
        self.usb.append(self.pin)
        self.usb.append(self.token)
        row = Gtk.Box(spacing=8)
        row.append(Gtk.Label(label='USB base port — كما يظهر على الهاتف'))
        row.append(self.port)
        self.usb.append(row)
        box.append(self.usb)
        self.wifi = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self.wifi.append(Gtk.Label(label='وصل الهاتف والكمبيوتر بنفس الشبكة. بعد بدء المستقبل،\nامسح كود الاقتران بكاميرا الآيفون ثم اضغط Connect over Wi-Fi.', xalign=0, wrap=True))
        self.address = Gtk.Entry(placeholder_text='عنوان الكمبيوتر داخل الشبكة')
        try:
            route = json.loads(subprocess.check_output(['ip', '-j', 'route', 'get', '1.1.1.1'], timeout=2))
            self.address.set_text(route[0]['prefsrc'])
        except (OSError, subprocess.SubprocessError, ValueError, KeyError, IndexError):
            pass
        self.wifi.append(self.address)
        box.append(self.wifi)
        self.preview = Gtk.CheckButton(label='عرض الصورة على الكمبيوتر')
        self.preview.set_active(True)
        self.webcam = Gtk.CheckButton(label='تشغيل كاميرا TitanCam الافتراضية')
        self.webcam.set_active(Path('/dev/video42').exists())
        box.append(self.preview)
        box.append(self.webcam)
        buttons = Gtk.Box(spacing=10)
        self.start_button = Gtk.Button(label='بدء الاتصال / تجديد الاقتران')
        self.start_button.add_css_class('suggested-action')
        self.start_button.connect('clicked', self.start)
        stop = Gtk.Button(label='إيقاف الاتصال')
        stop.connect('clicked', lambda *_: self.stop())
        buttons.append(self.start_button)
        buttons.append(stop)
        box.append(buttons)
        self.picture = Gtk.Picture()
        self.picture.set_size_request(300, 300)
        self.picture.set_can_shrink(True)
        self.picture.set_visible(False)
        box.append(self.picture)
        self.pairing = Gtk.Label(xalign=0, wrap=True, selectable=True)
        self.pairing.set_max_width_chars(70)
        box.append(self.pairing)
        self.metrics = Gtk.Label(xalign=0, wrap=True)
        box.append(self.metrics)
        self.mode.connect('notify::selected', self.mode_changed)
        self.mode_changed()
        GLib.timeout_add(500, self.refresh)
        self.window.present()

    def mode_changed(self, *_):
        usb = self.mode.get_selected() == 0
        self.usb.set_visible(usb)
        self.wifi.set_visible(not usb)
        self.picture.set_visible(False)
        self.pairing.set_text('')

    def start(self, *_):
        profile = ['saver', 'balanced', 'maximum'][self.profile.get_selected()]
        args = ['/usr/bin/titan-receiver']
        if self.mode.get_selected() == 0:
            pin = ''.join(self.pin.get_text().split()).replace(':', '').lower()
            token = ''.join(self.token.get_text().split()).lower()
            if not all(len(v) == 64 and all(c in '0123456789abcdef' for c in v) for v in (pin, token)):
                self.status.set_text('انقل بصمة الهاتف ورمز الاقتران كاملين: 64 حرفًا لكل خانة.')
                return
            args += ['usb', '--phone-pin', pin, '--pair-token', token, '--usb-base-port', str(self.port.get_value_as_int())]
        else:
            try:
                address = ipaddress.ip_address(self.address.get_text().strip())
                if address.is_unspecified or address.is_loopback or address.is_multicast:
                    raise ValueError()
            except ValueError:
                self.status.set_text('أدخل عنوان الكمبيوتر الصحيح داخل الشبكة المحلية.')
                return
            args += ['run', '--bind', str(address), '--advertise', str(address), '--pairing']
        args += ['--profile', profile]
        if self.preview.get_active():
            args.append('--preview')
        if self.webcam.get_active():
            if not os.access('/dev/video42', os.W_OK):
                self.status.set_text('الكاميرا الافتراضية /dev/video42 غير جاهزة؛ ألغِ اختيارها أو أكمل إعدادها.')
                return
            args += ['--webcam', '/dev/video42']
        self.stop()
        self.started = time.time()
        self.generation += 1
        current = self.generation
        env = dict(os.environ, RUST_LOG='info', NO_COLOR='1')
        try:
            self.process = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
        except OSError:
            self.status.set_text('تعذر تشغيل المستقبل؛ تحقق من تثبيت حزمة TitanCam.')
            return
        self.token.set_text('')
        self.status.set_text('المستقبل يعمل — انتظار اقتران الهاتف والصورة والصوت')
        self.metrics.set_text('')
        threading.Thread(target=self.read_output, args=(self.process, current), daemon=True).start()

    def stop(self):
        self.generation += 1
        process, self.process = self.process, None
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        if hasattr(self, 'status'):
            self.status.set_text('تم إيقاف الاتصال')
            self.metrics.set_text('')
            self.pairing.set_text('')
            self.picture.set_visible(False)
        (STATE / 'wifi-pairing.png').unlink(missing_ok=True)

    def read_output(self, process, current):
        path = STATE / 'desktop-test.log'
        def open_log():
            return os.fdopen(os.open(path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600), 'wb')
        output, size = open_log(), 0
        try:
            while data := process.stdout.readline(8192):
                if data.startswith(b'titancam://pair?'):
                    GLib.idle_add(self.show_pairing, data.decode('ascii').strip(), current)
                    continue  # Pairing secrets never enter diagnostic logs.
                if size + len(data) > 2 * 1024 * 1024:
                    output.close()
                    os.replace(path, STATE / 'desktop-test.previous.log')
                    output, size = open_log(), 0
                output.write(data)
                output.flush()
                size += len(data)
            status = process.wait()
            GLib.idle_add(self.ended, current, status)
        finally:
            output.close()

    def show_pairing(self, uri, current):
        if current != self.generation:
            return False
        path = STATE / 'wifi-pairing.png'
        fd = os.open(path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
        os.close(fd)
        try:
            subprocess.run(['qrencode', '-o', str(path), '-s', '6', '-m', '4'], input=uri.encode('ascii'), check=True, timeout=3, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.picture.set_filename(str(path))
            self.picture.set_visible(True)
            self.pairing.set_text('امسح الكود بكاميرا الآيفون خلال دقيقتين، ثم اضغط Connect over Wi-Fi في TitanCam.\nإذا انتهت المهلة اضغط بدء الاتصال لتجديد الاقتران.')
        except (OSError, subprocess.SubprocessError):
            self.pairing.set_text(uri)
        self.pairing_deadline = time.monotonic() + 120
        return False

    def ended(self, current, status):
        if current == self.generation:
            self.process = None
            self.status.set_text('توقفت الجلسة؛ راجع رسالة التطبيق أو سجل الفحص المحلي.' if status else 'تم إيقاف الاتصال')
        return False

    def refresh(self):
        if self.process and self.process.poll() is None:
            path = STATE / 'stats.json'
            try:
                if path.stat().st_mtime >= self.started and path.stat().st_size < 65536:
                    data = json.loads(path.read_text())
                    video, audio = data.get('video', 0), data.get('audio', 0)
                    if video and audio:
                        self.status.set_text('الصورة والصوت يصلان من الهاتف — ' + str(data.get('profile', '')))
                    self.metrics.set_text(f"فريمات الفيديو: {video} · حزم الصوت: {audio}\nDecoder: {data.get('decoder', '—')} · dropped: {data.get('dropped', 0)}")
            except (OSError, ValueError):
                pass
            if hasattr(self, 'pairing_deadline') and time.monotonic() >= self.pairing_deadline:
                self.picture.set_visible(False)
                (STATE / 'wifi-pairing.png').unlink(missing_ok=True)
        return True

Application().run(None)
