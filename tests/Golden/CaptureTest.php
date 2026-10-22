<?php

namespace Tests\Golden;

use App\Http\Livewire\Projects;
use App\Models\Cm;
use App\Models\Cmlog;
use App\Models\Image;
use App\Models\Label;
use App\Models\Project;
use App\Models\Script;
use App\Models\Setting;
use App\Models\User;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Http\UploadedFile;
use Illuminate\Support\Carbon;
use Laravel\Sanctum\Sanctum;
use Tests\TestCase;

/*
 * Not assertions: records the legacy app's observable behaviour as golden files,
 * which the Rust rewrite's test suite replays to prove byte-level parity.
 */
class CaptureTest extends TestCase
{
    use RefreshDatabase;

    const SERIAL = '10000000abcdef12';
    const OUT = __DIR__.'/../fixtures';

    protected $host = '172.20.0.1';

    protected function setUp(): void
    {
        parent::setUp();
        Carbon::setTestNow(Carbon::create(2024, 1, 2, 3, 4, 5, 'UTC'));
        $this->withServerVariables(['REMOTE_ADDR' => '172.20.0.5']);
    }

    // Laravel's test client derives HTTP_HOST from the URL, not from server variables.
    protected function url($query)
    {
        return "http://{$this->host}/scriptexecute?$query";
    }

    protected function write($relative, $contents)
    {
        $path = self::OUT.'/'.$relative;
        @mkdir(dirname($path), 0755, true);
        file_put_contents($path, $contents);
    }

    protected function pretty($data)
    {
        return json_encode($data, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES)."\n";
    }

    protected function state()
    {
        $cm = Cm::where('serial', self::SERIAL)->first();
        return [
            'cm' => $cm ? collect($cm->getAttributes())->except(['id', 'created_at', 'updated_at'])->all() : null,
            'cmlogs' => Cmlog::orderBy('id')->get()->map(fn ($l) => [
                'msg' => preg_replace("#'[^' ]*/label-[A-Za-z0-9]+'#", "'LABELFILE'", $l->msg),
            ] + $l->only(['cm', 'board', 'ip', 'loglevel']))->all(),
        ];
    }

    protected function capture($name, $response)
    {
        $body = preg_replace('/^echo "[A-Za-z0-9]{64}" >\/dev\/urandom$/m', 'echo "RANDOM64" >/dev/urandom', $response->getContent());
        $this->write("scriptexecute/$name.sh", $body);
        $this->write("scriptexecute/$name.state.json", $this->pretty([
            'status' => $response->getStatusCode(),
            'content_type' => $response->headers->get('Content-Type'),
        ] + $this->state()));
        $this->assertTrue(true);
    }

    protected function image($ext, $uncompressedSize = 1073741824, $uncompressedSha = 'default')
    {
        if ($uncompressedSha === 'default')
            $uncompressedSha = 'aa'.str_repeat('0', 62);

        return Image::forceCreate([
            'filename' => "raspios.img.$ext",
            'filename_on_server' => "abcdefghijklmnopqrstuvwxyz0123456789ABCD.$ext",
            'filename_extension' => $ext,
            'sha256' => 'bb'.str_repeat('0', 62),
            'uncompressed_sha256' => $uncompressedSha,
            'uncompressed_size' => $uncompressedSize,
        ]);
    }

    protected function script($name, $type, $priority = 100, $bg = false)
    {
        return Script::create([
            'name' => $name,
            'script_type' => $type,
            'priority' => $priority,
            'bg' => $bg,
            'script' => "#!/bin/sh\necho '$name' \"\$STORAGE\"\n",
        ]);
    }

    protected function project($attrs = [], $scripts = [])
    {
        $p = Project::create($attrs + [
            'name' => 'Golden',
            'device' => 'cm4',
            'storage' => '/dev/mmcblk0',
            'label_moment' => 'never',
            'verify' => false,
        ]);
        $p->scripts()->sync(collect($scripts)->pluck('id'));
        Setting::updateOrCreate(['key' => 'active_project'], ['value' => $p->id]);
        return $p;
    }

    protected function start($query = [])
    {
        return $this->get($this->url(http_build_query($query + [
            'serial' => self::SERIAL,
            'model' => 'CM4',
            'memorysize' => '3884000',
            'storagesize' => '61071360',
            'cid' => '15010038474e443352',
            'csd' => 'd00f00320f5903ff',
            'temp' => '45.2',
        ])));
    }

    public function test_missing_serial()
    {
        $this->capture('missing_serial', $this->get("http://{$this->host}/scriptexecute"));
    }

    public function test_no_project()
    {
        $this->capture('no_project', $this->start());
    }

    public function test_gz_with_scripts()
    {
        $this->project(['image_id' => $this->image('gz')->id], [
            $this->script('pre-b', 'preinstall', 50),
            $this->script('pre-a', 'preinstall', 50),
            $this->script('pre-late', 'preinstall', 200),
            $this->script('post-bg', 'postinstall', 100, true),
        ]);
        $this->capture('gz_with_scripts', $this->start(['mac' => 'dc:a6:32:01:02:03']));
    }

    public function test_xz_bootmode3_inversejumper()
    {
        $this->project(['image_id' => $this->image('xz')->id]);
        $this->capture('xz_bootmode3_inversejumper', $this->start(['bootmode' => '3', 'inversejumper' => '1101']));
    }

    public function test_bz2_sda_without_storagesize()
    {
        $this->project(['image_id' => $this->image('bz2')->id, 'storage' => '/dev/sda']);
        $this->capture('bz2_sda_without_storagesize', $this->start(['storagesize' => '']));
    }

    public function test_scripts_without_image()
    {
        $this->project([], [$this->script('post-only', 'postinstall')]);
        $this->capture('scripts_without_image', $this->start());
    }

    public function test_eeprom_and_verify_mib_aligned()
    {
        Setting::create(['key' => 'active_eeprom_sha256', 'value' => 'cc'.str_repeat('0', 62)]);
        $this->project([
            'image_id' => $this->image('gz')->id,
            'eeprom_firmware' => 'stable/pieeprom-2024-01-01.bin',
            'verify' => true,
        ], [$this->script('post-a', 'postinstall')]);
        $this->capture('eeprom_and_verify_mib_aligned', $this->start());
    }

    public function test_verify_sector_aligned()
    {
        $this->project(['image_id' => $this->image('xz', 1048576 + 512)->id, 'verify' => true]);
        $this->capture('verify_sector_aligned', $this->start());
    }

    public function test_ipv6_host()
    {
        $this->project(['image_id' => $this->image('gz')->id]);
        $this->host = '[fe80::1]';
        $this->capture('ipv6_host', $this->start());
    }

    public function test_label_preinstall_command_failure()
    {
        $label = Label::create([
            'name' => 'Cmd',
            'printer_type' => 'command',
            'print_command' => 'false $file',
            'file_extension' => 'txt',
            'template' => '$mac $serial $provisionboard',
        ]);
        $this->project(['image_id' => $this->image('gz')->id, 'label_id' => $label->id, 'label_moment' => 'preinstall']);
        $this->capture('label_preinstall_command_failure', $this->start());
    }

    public function test_error_verify_without_uncompressed_sha()
    {
        $this->project(['image_id' => $this->image('gz', null, null)->id, 'verify' => true]);
        $this->capture('error_verify_without_uncompressed_sha', $this->start());
    }

    public function test_error_not_sector_multiple()
    {
        $this->project(['image_id' => $this->image('gz', 1000)->id, 'verify' => true]);
        $this->capture('error_not_sector_multiple', $this->start());
    }

    public function test_error_missing_emmc()
    {
        $this->project(['image_id' => $this->image('gz')->id]);
        $this->capture('error_missing_emmc', $this->start(['storagesize' => '']));
    }

    public function test_error_image_too_big()
    {
        $this->project(['image_id' => $this->image('gz', 64 * 1024 * 1024 * 1024)->id]);
        $this->capture('error_image_too_big', $this->start());
    }

    public function test_followup_requests()
    {
        $this->project(['image_id' => $this->image('gz')->id], [$this->script('pre-a', 'preinstall')]);
        $this->start();

        $this->capture('followup_eeprom_version', $this->post($this->url('serial='.self::SERIAL), [
            'eeprom_version' => UploadedFile::fake()->createWithContent('eeprom_version', "VERSION:abcdef\nBUILD_TIMESTAMP=1700000000\n"),
        ]));

        $this->capture('followup_preinstall_ok', $this->post($this->url('serial='.self::SERIAL.'&retcode=0&phase=preinstall'), [
            'log' => UploadedFile::fake()->createWithContent('pre.log', "pre output\n"),
        ]));

        $this->capture('followup_dd_failed', $this->post($this->url('serial='.self::SERIAL.'&retcode=1&phase=dd'), [
            'log' => UploadedFile::fake()->createWithContent('dd.log', "dd: error writing\n"),
        ]));

        $this->capture('followup_alldone', $this->get($this->url('serial='.self::SERIAL.'&alldone=1&temp=50.1&verify=1')));
    }

    public function test_followup_unknown_serial()
    {
        $this->capture('followup_unknown_serial', $this->get($this->url('serial=deadbeef&alldone=1')));
    }

    public function test_api()
    {
        $image = $this->image('gz');
        $label = Label::create([
            'name' => 'Ftp',
            'printer_type' => 'ftp',
            'ftp_hostname' => 'printer',
            'ftp_username' => 'print',
            'ftp_password' => 'secret',
            'file_extension' => 'jscript',
            'template' => '$mac',
        ]);
        $script = $this->script('pre-a', 'preinstall');
        $project = $this->project(['image_id' => $image->id, 'label_id' => $label->id], [$script]);
        $this->start();

        Sanctum::actingAs(User::factory()->create(), ['read', 'update']);

        $endpoints = [
            'cms' => '/api/cms',
            'project_cms' => "/api/projects/{$project->id}/cms",
            'projects' => '/api/projects',
            'images' => '/api/images',
            'image' => "/api/images/{$image->id}",
            'image_missing' => '/api/images/999',
            'firmware' => '/api/firmware',
            'scripts' => '/api/scripts',
            'labels' => '/api/labels',
            'project' => "/api/projects/{$project->id}",
        ];
        foreach ($endpoints as $name => $url) {
            $r = $this->getJson($url);
            $this->write("api/$name.json", $this->pretty(['status' => $r->getStatusCode(), 'body' => json_decode($r->getContent())]));
        }

        $r = $this->patchJson("/api/projects/{$project->id}", ['name' => 'Renamed', 'verify' => true]);
        $this->write('api/project_patch.json', $this->pretty(['status' => $r->getStatusCode(), 'body' => json_decode($r->getContent())]));

        $r = $this->deleteJson("/api/images/{$image->id}");
        $this->write('api/image_delete_forbidden.json', $this->pretty(['status' => $r->getStatusCode(), 'body' => json_decode($r->getContent())]));

        $this->assertTrue(true);
    }

    public function test_eeprom_patching()
    {
        $source = self::OUT.'/eeprom/pieeprom.bin';
        if (!is_readable($source)) {
            $this->markTestSkipped("Place an upstream pieeprom-*.bin at $source to capture EEPROM fixtures");
        }

        $data = file_get_contents($source);
        $p = new Projects;
        $this->write('eeprom/bootconf.txt', $p->getEepromSettings($data));

        $settings = "[all]\nBOOT_UART=1\nBOOT_ORDER=0xf21\nPOWER_OFF_ON_HALT=1\n";
        $this->assertTrue($p->setEepromSettings($data, $settings));
        $this->write('eeprom/patched-settings.txt', $settings);
        $this->write('eeprom/pieeprom-patched.bin', $data);
    }
}
