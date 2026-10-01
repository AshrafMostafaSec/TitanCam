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
import importlib.util
from concurrent.futures import ThreadPoolExecutor
_control_spec = importlib.util.spec_from_file_location('titancam_control', Path(__file__).with_name('desktop-control.py'))
_control = importlib.util.module_from_spec(_control_spec)
_control_spec.loader.exec_module(_control)

gi.require_version('Gtk', '4.0')
from gi.repository import GLib, Gtk

STATE = Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local/state')) / 'titancam'

class Application(Gtk.Application):
    def __init__(self):
        super().__init__(application_id='com.ashrafmostafasec.titancam.receiver')
        self.process = None
        self.process_worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix='titancam-process')
        self.owned_process = None  # Access only from the serialized process worker.
        self.generation = 0
        self.started = 0
        self.gpu_at = 0
        self.gpu_pending = False
        self.poll_pending = False
        self.syncing = False
        self.last_session = None
        self.capability_key = None
        self.cameras = []
        self.microphones = []
        self.sources = []
        self.formats = []
        self.capture_config = {}
        self.connected = False
        self.live_controls = False
        self.current_address = None
        self.config_pending = False
        self.gain_timer = None
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
        self.window.set_icon_name('titancam')
        self.window.set_default_size(700, 800)
        self.hold()
        self.window.connect('close-request', self.hide_window)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        for side in ('top', 'bottom', 'start', 'end'):
            getattr(box, 'set_margin_' + side)(22)
        scroller = Gtk.ScrolledWindow()
        scroller.set_child(box)
        self.window.set_child(scroller)
        title = Gtk.Label(label='TitanCam · iPhone → Linux', xalign=0)
        title.add_css_class('title-1')
        box.append(title)
        self.status = Gtk.Label(label='جاهز لاختبار الكاميرا والصوت', xalign=0, wrap=True)
        box.append(self.status)
        box.append(Gtk.Label(label='افتح TitanCam على الآيفون: اختر اسم الكمبيوتر على Wi-Fi،\nأو وصل الكابل واضغط Connect with USB. الاتصال يتم تلقائيًا.', xalign=0, wrap=True))
        self.profile = Gtk.DropDown.new_from_strings(['Saver — 720p30', 'Balanced — 1080p60', 'Maximum — 4K60 تجريبي'])
        self.profile.set_selected(1)
        box.append(self.profile)
        profile_apply = Gtk.Button(label='تطبيق وضع الجودة بدون فصل الاتصال')
        profile_apply.connect('clicked', self.apply_profile)
        box.append(profile_apply)
        self.camera = Gtk.DropDown.new_from_strings(['انتظار معلومات كاميرات الآيفون'])
        self.camera.connect('notify::selected', self.camera_changed)
        box.append(Gtk.Label(label='الكاميرا والعدسة', xalign=0)); box.append(self.camera)
        self.format = Gtk.DropDown.new_from_strings(['انتظار الدقات المتاحة'])
        box.append(Gtk.Label(label='الدقة ومعدل الفريمات', xalign=0)); box.append(self.format)
        self.codec = Gtk.DropDown.new_from_strings(['H.264 — توافق واسع', 'HEVC — ضغط أكثر كفاءة، يحتاج اختبار'])
        self.codec.set_selected(1)
        box.append(self.codec)
        self.wifi_budget = Gtk.SpinButton.new_with_range(5, 200, 5)
        self.wifi_budget.set_value(35)
        box.append(Gtk.Label(label="حد إرسال Wi-Fi بالميجابت/ثانية — 35 كبداية لشبكة 2.4GHz", xalign=0, wrap=True))
        box.append(self.wifi_budget)
        self.microphone = Gtk.DropDown.new_from_strings(['انتظار مصادر الميكروفون'])
        self.microphone.connect('notify::selected', self.microphone_changed)
        box.append(Gtk.Label(label='مدخل الميكروفون', xalign=0)); box.append(self.microphone)
        self.source = Gtk.DropDown.new_from_strings(['اختيار النظام'])
        box.append(Gtk.Label(label='مكان الميكروفون — حسب المصادر التي يتيحها iOS', xalign=0)); box.append(self.source)
        self.capture_apply = Gtk.Button(label='تطبيق الكاميرا / الدقة / الميكروفون')
        self.capture_apply.connect('clicked', self.apply_capture)
        self.capture_apply.set_sensitive(False)
        box.append(self.capture_apply)
        self.gain = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, -60, 12, 1)
        self.gain.set_value(0); self.gain.connect('value-changed', self.gain_changed)
        box.append(Gtk.Label(label='مستوى صوت TitanCam بالديسيبل — 0 = المستوى الأصلي', xalign=0)); box.append(self.gain)
        self.mute = Gtk.CheckButton(label='كتم الصوت مع استمرار الميكروفون')
        self.mute.connect('toggled', lambda *_: self.send_audio())
        box.append(self.mute)
        self.mirror = Gtk.CheckButton(label='Mirror — قلب الصورة أفقيًا')
        self.flip = Gtk.CheckButton(label='Flip — قلب الصورة رأسيًا')
        for control in (self.mirror, self.flip):
            control.connect('toggled', lambda *_: self.send_transform()); box.append(control)
        self.action_status = Gtk.Label(xalign=0, wrap=True)
        box.append(self.action_status)

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
        self.start_button = Gtk.Button(label='تشغيل / إعادة تجهيز مخارج المستقبل')
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

    def command(self, command, body=None, capture=False):
        if capture and self.config_pending:
            self.action_status.set_text('تغيير إعدادات الكاميرا قيد التنفيذ؛ انتظر التأكيد')
            return
        if capture:
            self.config_pending = True; self.capture_apply.set_sensitive(False)
        generation = self.generation
        def worker():
            try:
                result = _control.request(command, body)
                GLib.idle_add(self.command_done, generation, capture, result, None)
            except (OSError, ValueError) as error:
                GLib.idle_add(self.command_done, generation, capture, None, str(error))
        threading.Thread(target=worker, daemon=True).start()

    def command_done(self, generation, capture, result, error):
        if generation != self.generation:
            return False
        if capture:
            self.config_pending = False; self.capture_apply.set_sensitive(self.connected and self.live_controls and bool(self.cameras))
        self.action_status.set_text(('لم يطبق التغيير: ' + error) if error else 'تم تطبيق الإعدادات وتأكيدها')
        return False

    def apply_profile(self, *_):
        self.command('SetProfile', {'profile': ['saver', 'balanced', 'maximum'][self.profile.get_selected()], 'codec': ['h264', 'hevc'][self.codec.get_selected()], 'wifi_budget_mbps': self.wifi_budget.get_value_as_int()}, capture=True)

    def apply_capture(self, *_):
        camera_index = self.camera.get_selected(); format_index = self.format.get_selected()
        if not (0 <= camera_index < len(self.cameras) and 0 <= format_index < len(self.formats)):
            return
        camera = self.cameras[camera_index]; fmt = self.formats[format_index]
        codec = ['h264', 'hevc'][self.codec.get_selected()]
        profile = self.capture_config.get('profile', 'balanced')
        rates = {'saver':3000000,'balanced':8000000,'maximum':24000000} if codec == 'hevc' else {'saver':4000000,'balanced':14000000,'maximum':65000000}
        patch = dict(fmt, camera_id=camera['id'], codec=codec, bitrate=rates.get(profile,8000000), wifi_budget_mbps=self.wifi_budget.get_value_as_int())
        mic_index = self.microphone.get_selected(); source_index = self.source.get_selected()
        if 0 <= mic_index < len(self.microphones):
            source = self.sources[source_index] if 0 <= source_index < len(self.sources) else None
            patch.update(audio_input_id=self.microphones[mic_index]['id'], audio_data_source=source['id'] if source else None)
        self.command('SetConfig', patch, capture=True)

    @staticmethod
    def dropdown(widget, labels):
        widget.set_model(Gtk.StringList.new(labels or ['غير متاح']))
        widget.set_selected(0)

    def camera_changed(self, *_):
        if self.syncing or not self.cameras:
            return
        index = self.camera.get_selected()
        self.formats = self.cameras[index].get('formats', []) if 0 <= index < len(self.cameras) else []
        self.dropdown(self.format, [f"{m['width']}×{m['height']} · {m['fps']} FPS" for m in self.formats])
        cfg = self.capture_config
        match = next((i for i, mode in enumerate(self.formats) if all(mode.get(k) == cfg.get(k) for k in ('width', 'height', 'fps'))), 0)
        self.format.set_selected(match)

    def microphone_changed(self, *_):
        if self.syncing or not self.microphones:
            return
        index = self.microphone.get_selected()
        self.sources = [{'id': None, 'name': 'اختيار النظام'}] + (self.microphones[index].get('sources', []) if 0 <= index < len(self.microphones) else [])
        self.dropdown(self.source, [f"{m['name']} · {m.get('location', '')} {m.get('orientation', '')}" for m in self.sources])
        self.source.set_selected(next((i for i, source in enumerate(self.sources) if source['id'] == self.capture_config.get('audio_data_source')), 0))

    def gain_changed(self, *_):
        if self.syncing:
            return
        if self.gain_timer:
            GLib.source_remove(self.gain_timer)
        self.gain_timer = GLib.timeout_add(150, self.send_audio)

    def send_audio(self, *_):
        self.gain_timer = None
        if not self.syncing:
            self.command('SetAudio', {'gain_db': self.gain.get_value(), 'mute': self.mute.get_active()})
        return False

    def send_transform(self):
        if not self.syncing:
            self.command('SetTransform', {'mirror': self.mirror.get_active(), 'flip': self.flip.get_active()})

    def poll_status(self):
        if self.poll_pending:
            return
        self.poll_pending = True
        generation = self.generation
        def worker():
            try:
                data = _control.request('GetStatus')
            except (OSError, ValueError):
                data = None
            address = self.local_address()
            GLib.idle_add(self.apply_address, generation, address)
            GLib.idle_add(self.apply_status, generation, data)
        threading.Thread(target=worker, daemon=True).start()

    def apply_address(self, generation, address):
        if generation != self.generation:
            return False
        self.current_address = address
        if self.process and self.process.poll() is None and address != self.active_address:
            self.start()
        return False

    def apply_status(self, generation, data):
        self.poll_pending = False
        if generation != self.generation:
            return False
        if data is None:
            data = {'connected': False}
        self.connected = bool(data.get('connected'))
        self.live_controls = bool(data.get("capabilities", {}).get("live_controls"))
        self.capture_apply.set_sensitive(self.connected and self.live_controls and bool(data.get("capabilities", {}).get("cameras")) and not self.config_pending)
        self.capture_config = data.get('config', {})
        cap = data.get('capabilities', {})
        key = json.dumps(cap, sort_keys=True) + str(data.get('session')) + str(self.capture_config.get('config_id'))
        if key != self.capability_key:
            self.capability_key = key
            self.cameras = cap.get('cameras', []); self.microphones = cap.get('audio_inputs', [])
            self.formats = []; self.sources = []
            self.syncing = True
            self.dropdown(self.camera, [f"{c['name']} · {c['position']}" for c in self.cameras])
            self.dropdown(self.microphone, [m['name'] for m in self.microphones])
            self.camera.set_selected(next((i for i, c in enumerate(self.cameras) if c['id'] == self.capture_config.get('camera_id')), 0))
            self.microphone.set_selected(next((i for i, m in enumerate(self.microphones) if m['id'] == self.capture_config.get('audio_input_id')), 0))
            self.codec.set_selected(1 if self.capture_config.get('codec') == 'hevc' else 0)
            self.profile.set_selected({'saver':0,'balanced':1,'maximum':2}.get(self.capture_config.get('profile'),1))
            self.wifi_budget.set_value(self.capture_config.get('wifi_budget_mbps',35))
            self.syncing = False
            self.camera_changed(); self.microphone_changed()
        if not self.connected:
            self.status.set_text('الكمبيوتر جاهز — في انتظار الآيفون' if self.process and self.process.poll() is None else 'تم إيقاف المستقبل')
            self.metrics.set_text(''); self.gpu.set_text('')
            if data.get('last_error'):
                self.action_status.set_text('سبب انقطاع الجلسة: ' + data['last_error'])
        if self.connected:
            cfg = self.capture_config
            stats = data.get('stats', {})
            state = data.get('capabilities_state', 'ready' if cap.get('live_controls') else 'update_required')
            if state != 'ready':
                reason = {'update_required':'تطبيق الآيفون لم يعلن دعم التحكم؛ ثبّت نسخة 0.3.1 build 8', 'pending':'جارٍ استلام إمكانيات الكاميرا والمايك من الآيفون', 'unavailable':'لم تصل إمكانيات الكاميرا والمايك؛ أعد الاتصال بعد تحديث الآيفون'}.get(state, 'إمكانيات الهاتف غير متاحة')
                self.action_status.set_text(reason)
                self.syncing = True
                self.dropdown(self.camera, [reason]); self.dropdown(self.format, ['الدقات لم تُرسل من الآيفون'])
                self.dropdown(self.microphone, ['مصادر المايك لم تُرسل من الآيفون']); self.dropdown(self.source, ['غير متاح'])
                self.syncing = False
            sender = data.get('sender', {})
            version = sender.get('app_version') or 'غير معلن'
            build = sender.get('app_build') or '؟'

            self.status.set_text(f"متصل · {stats.get('transport', '')} · {cfg.get('width')}×{cfg.get('height')} · {cfg.get('fps')} FPS target · {cfg.get('codec', '—').upper()} · iPhone {version}/{build}")
            self.metrics.set_text(f"Decoder: {stats.get('decoder', '—')} · decoded {stats.get('decoded_fps', 0):.1f} FPS\nDropped {stats.get('dropped', 0)} · recoveries {stats.get('recoveries', 0)} · audio underruns {stats.get('microphone_underruns', 0)}\nAudio peak {stats.get('audio_peak', 0):.2f} · clipping {stats.get('audio_clipped', 0)} · RTT {stats.get('clock_rtt_ms', 0):.1f} ms")
            sender_stats = stats.get('sender_stats')
            if isinstance(sender_stats, dict):
                self.metrics.set_text(self.metrics.get_text() + f"\nفقد عند التقاط الهاتف {sender_stats.get('capture_dropped', 0)} · تخطي المشفّر {sender_stats.get('encoder_skipped', 0)}")
            if state == 'ready' and cfg.get('fallback_reason'):
                self.action_status.set_text(cfg['fallback_reason'])
        return False

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
        address = self.current_address
        # USB remains usable even when no Wi-Fi network is available.
        args = [os.environ.get('TITANCAM_RECEIVER', '/usr/bin/titan-receiver'), 'local', '--bind', '0.0.0.0']
        if address:
            args += ['--advertise', address]
        self.active_address = address
        self.network.set_text('Wi-Fi: ' + address if address else 'USB جاهز — لا توجد شبكة Wi-Fi حاليًا')
        args += ['--profile', profile, '--codec', ['h264','hevc'][self.codec.get_selected()], '--wifi-budget-mbps', str(self.wifi_budget.get_value_as_int())]
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
        self.status.set_text('جاري تجهيز المستقبل…')
        self.metrics.set_text('')
        self.config_pending = False
        self.capability_key = None
        def launch():
            self.terminate_owned()
            try:
                process = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
                self.owned_process = process
                GLib.idle_add(self.launched, current, process)
            except OSError:
                GLib.idle_add(self.launch_failed, current)
        self.process_worker.submit(launch)

    def launched(self, current, process):
        if current == self.generation:
            self.process = process
            threading.Thread(target=self.read_output, args=(process, current), daemon=True).start()
        return False

    def launch_failed(self, current):
        if current == self.generation:
            self.status.set_text('تعذر تشغيل المستقبل؛ تحقق من تثبيت حزمة TitanCam.')
        return False

    def stop(self):
        self.generation += 1
        self.process = None
        self.connected = False
        self.process_worker.submit(self.terminate_owned)
        if hasattr(self, 'capture_apply'):
            self.capture_apply.set_sensitive(False)
        if hasattr(self, 'status'):
            self.status.set_text('تم إيقاف المستقبل')
            self.metrics.set_text('')
            self.gpu.set_text('')

    def terminate_owned(self):
        process, self.owned_process = self.owned_process, None
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
            GLib.idle_add(self.gpu.set_text, "NVIDIA غير متاح حاليًا — يلزم فحص التعريف قبل اختبار 4K.")
        finally:
            self.gpu_pending = False

    def refresh(self):
        if self.process and self.process.poll() is None:
            self.poll_status()
            if self.connected and not self.gpu_pending and time.monotonic() - self.gpu_at >= 5:
                self.gpu_at = time.monotonic(); self.gpu_pending = True
                threading.Thread(target=self.query_gpu, daemon=True).start()
        return True

if __name__ == '__main__':
    Application().run(None)
