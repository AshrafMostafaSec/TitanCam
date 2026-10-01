#!/usr/bin/env python3
"""Trusted LAN desktop controller with automatic discovery and cable bootstrap."""
import fcntl
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
        self.gpu_at = 0
        self.gpu_pending = False
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
        self.window.set_default_size(620, 460)
        self.hold()
        self.window.connect('close-request', self.hide_window)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        for side in ('top', 'bottom', 'start', 'end'):
            getattr(box, 'set_margin_' + side)(22)
        self.window.set_child(box)
        title = Gtk.Label(label='TitanCam · iPhone → Linux', xalign=0)
        title.add_css_class('title-1')
        box.append(title)
        self.status = Gtk.Label(label='جاهز لاختبار الكاميرا والصوت', xalign=0, wrap=True)
        box.append(self.status)
        box.append(Gtk.Label(label='افتح TitanCam على الآيفون: اختر اسم الكمبيوتر على Wi-Fi،\nأو وصل الكابل واضغط Connect with USB. الاتصال يتم تلقائيًا.', xalign=0, wrap=True))
        self.profile = Gtk.DropDown.new_from_strings(['Saver — 720p30', 'Balanced — 1080p60', 'Maximum — 4K60 تجريبي'])
        box.append(self.profile)
        self.network = Gtk.Label(xalign=0, wrap=True)
        box.append(self.network)
        self.preview = Gtk.CheckButton(label='عرض الصورة على الكمبيوتر')
        self.preview.set_active(True)
        self.webcam = Gtk.CheckButton(label='تشغيل كاميرا TitanCam الافتراضية')
        self.webcam.set_active(Path('/dev/video42').exists())
        box.append(self.preview)
        box.append(self.webcam)
        webcam_ready = os.access('/dev/video42', os.W_OK)
        box.append(Gtk.Label(label='الكاميرا الافتراضية جاهزة — تظهر في برامج الكاميرا عند بدء وصول الفيديو.' if webcam_ready else 'الكاميرا الافتراضية غير جاهزة؛ يمكنك استقبال الصورة بدونها.', xalign=0, wrap=True))
        buttons = Gtk.Box(spacing=10)
        self.start_button = Gtk.Button(label='تشغيل / تطبيق الإعدادات')
        self.start_button.add_css_class('suggested-action')
        self.start_button.connect('clicked', self.start)
        stop = Gtk.Button(label='إيقاف المستقبل')
        stop.connect('clicked', lambda *_: self.stop())
        buttons.append(self.start_button)
        buttons.append(stop)
        box.append(buttons)
        quit_button = Gtk.Button(label='إغلاق TitanCam بالكامل')
        quit_button.connect('clicked', lambda *_: self.quit())
        buttons.append(quit_button)
        self.metrics = Gtk.Label(xalign=0, wrap=True)
        box.append(self.metrics)
        self.gpu = Gtk.Label(xalign=0, wrap=True)
        box.append(self.gpu)
        GLib.timeout_add(1000, self.refresh)
        self.window.present()
        self.start()

    def hide_window(self, *_):
        self.window.set_visible(False)
        return True  # Receiver stays discoverable when its window is closed.

    @staticmethod
    def local_address():
        try:
            route = json.loads(subprocess.check_output(['ip', '-j', '-4', 'route', 'show', 'default'], timeout=2))
            interface = route[0]['dev']
            addresses = json.loads(subprocess.check_output(['ip', '-j', '-4', 'addr', 'show', 'dev', interface], timeout=2))
            return next(x['local'] for d in addresses for x in d['addr_info'] if x.get('scope') == 'global')
        except (OSError, subprocess.SubprocessError, ValueError, KeyError, IndexError, StopIteration):
            return None

    def start(self, *_):
        profile = ['saver', 'balanced', 'maximum'][self.profile.get_selected()]
        address = self.local_address()
        # USB remains usable even when no Wi-Fi network is available.
        args = [os.environ.get('TITANCAM_RECEIVER', '/usr/bin/titan-receiver'), 'local', '--bind', '0.0.0.0']
        if address:
            args += ['--advertise', address]
        self.active_address = address
        self.network.set_text('Wi-Fi: ' + address if address else 'USB جاهز — لا توجد شبكة Wi-Fi حاليًا')
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
        self.status.set_text('جاري تجهيز المستقبل…')
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
            self.status.set_text('تم إيقاف المستقبل')
            self.metrics.set_text('')
            self.gpu.set_text('')

    def read_output(self, process, current):
        path = STATE / 'desktop-test.log'
        def open_log():
            return os.fdopen(os.open(path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600), 'wb')
        output, size = open_log(), 0
        try:
            while data := process.stdout.readline(8192):
                if data.startswith(b'titancam://pair?'):
                    continue  # Pairing secrets never enter diagnostic logs.
                if b'listening on' in data:
                    GLib.idle_add(self.ready, current)
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

    def ready(self, current):
        if current == self.generation:
            self.status.set_text('الكمبيوتر جاهز — اختره على الآيفون أو اضغط اتصال USB')
        return False

    def ended(self, current, status):
        if current == self.generation:
            self.process = None
            self.status.set_text('توقفت الجلسة؛ راجع رسالة التطبيق أو سجل الفحص المحلي.' if status else 'تم إيقاف المستقبل')
        return False

    def query_gpu(self):
        try:
            values = subprocess.check_output(['nvidia-smi', '--query-gpu=utilization.gpu,utilization.decoder,power.draw', '--format=csv,noheader,nounits'], timeout=2, stderr=subprocess.DEVNULL).decode('ascii').splitlines()[0].split(',')
            if len(values) == 3:
                text = f'إنفيديا: استخدام عام {values[0].strip()}% · فك الفيديو {values[1].strip()}% · {values[2].strip()} وات'
                GLib.idle_add(self.gpu.set_text, text)
        except (OSError, subprocess.SubprocessError, ValueError, IndexError):
            pass
        finally:
            self.gpu_pending = False

    def refresh(self):
        if self.process and self.process.poll() is None:
            address = self.local_address()
            if address != self.active_address:
                self.start()
                return True
            path = STATE / 'stats.json'
            try:
                if path.stat().st_mtime >= self.started and time.time() - path.stat().st_mtime < 2.5 and path.stat().st_size < 65536:
                    data = json.loads(path.read_text())
                    video, audio = data.get('video', 0), data.get('audio', 0)
                    if video and not self.gpu_pending and time.monotonic() - self.gpu_at >= 5:
                        self.gpu_at = time.monotonic()
                        self.gpu_pending = True
                        threading.Thread(target=self.query_gpu, daemon=True).start()
                    if video and audio:
                        self.status.set_text('الصورة والصوت يصلان من الهاتف — ' + str(data.get('profile', '')))
                    self.metrics.set_text(f"فريمات الفيديو: {video} · حزم الصوت: {audio}\nDecoder: {data.get('decoder', '—')} · dropped: {data.get('dropped', 0)}")
            except (OSError, ValueError):
                pass
            if not path.exists() or time.time() - path.stat().st_mtime > 2.5:
                self.status.set_text('الكمبيوتر جاهز — في انتظار اتصال الآيفون')
                self.metrics.set_text('')
                self.gpu.set_text('')
        return True

Application().run(None)
