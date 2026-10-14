// The pure helpers the chat's device cards and the session's device strip
// read devices with (debug_devices.ts).
import { describe, it, expect } from 'vitest';
import {
  claimLeft,
  deviceCallLine,
  devicesOf,
  holderName,
  parseDeviceCall,
  parseDeviceResult,
  placeName,
  resolveDevice,
  sessionHolder,
  type DebugDevice,
} from './debug_devices';

const dev = (o: Partial<DebugDevice>): DebugDevice => ({
  id: 1,
  title: 'Pixel 9',
  host: 'mac',
  platform: 'android',
  kind: 'physical',
  key: 'usb:1',
  name: 'Pixel 9',
  state: 'online',
  ready: true,
  shared: false,
  first_seen_at: 1,
  last_seen_at: 2,
  ...o,
});

const pixel = dev({ id: 3, label: 'bench pixel', serial: 'R5CT', key: 'usb:R5CT' });
const macSim = dev({ id: 12, host: 'mac', platform: 'ios', kind: 'simulator', name: 'iPhone 16', title: 'iPhone 16', key: 'sim:A' });
const boxSim = dev({ id: 14, host: 'box', platform: 'ios', kind: 'simulator', name: 'iPhone 16', title: 'iPhone 16', key: 'sim:B' });
const all = [pixel, macSim, boxSim];

describe('resolveDevice', () => {
  it('reads `#12` as an id', () => {
    expect(resolveDevice(all, '#12')?.id).toBe(12);
    expect(resolveDevice(all, '#99')).toBeNull();
  });

  it('matches a label, serial or key, case-insensitively', () => {
    expect(resolveDevice(all, 'Bench Pixel')?.id).toBe(3);
    expect(resolveDevice(all, 'r5ct')?.id).toBe(3);
    expect(resolveDevice(all, 'usb:R5CT')?.id).toBe(3);
  });

  it('narrows by `host/that`', () => {
    expect(resolveDevice(all, 'box/iPhone 16')?.id).toBe(14);
    expect(resolveDevice(all, 'mac/iphone 16')?.id).toBe(12);
    expect(resolveDevice(all, 'nowhere/iPhone 16')).toBeNull();
  });

  it('answers null when a name is ambiguous or unknown', () => {
    expect(resolveDevice(all, 'iPhone 16')).toBeNull();
    expect(resolveDevice(all, 'Galaxy')).toBeNull();
  });

  it('takes a bare number as an id when nothing else matches', () => {
    expect(resolveDevice(all, '14')?.id).toBe(14);
    expect(resolveDevice(all, ' #3 ')?.id).toBe(3);
  });
});

describe('devicesOf', () => {
  it('splits what a holder holds from what it waits for, with its place', () => {
    const me = sessionHolder({ id: 42, host_alias: 'mercury' });
    expect(me).toBe('host:mercury#42');
    const held = dev({ id: 1, claimed_by: me });
    const queued = dev({
      id: 2,
      claimed_by: 'host:mac#7',
      queue: [
        { holder: 'host:mac#8', queued_at: 1 },
        { holder: me, queued_at: 2 },
      ],
    });
    const other = dev({ id: 3, claimed_by: 'host:mac#7' });
    const r = devicesOf([held, queued, other], me);
    expect(r.held.map((d) => d.id)).toEqual([1]);
    expect(r.waiting.map((w) => [w.device.id, w.place])).toEqual([[2, 2]]);
    expect(devicesOf([other], me)).toEqual({ held: [], waiting: [] });
  });
});

describe('words', () => {
  it('names a place in line', () => {
    expect([1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101].map(placeName)).toEqual([
      'next',
      '2nd in line',
      '3rd in line',
      '4th in line',
      '11th in line',
      '12th in line',
      '13th in line',
      '21st in line',
      '22nd in line',
      '23rd in line',
      '101st in line',
    ]);
  });

  it('names a holder', () => {
    expect(holderName('host:mercury#42')).toBe('session #42 on mercury');
    expect(holderName('host:mercury')).toBe('a session on mercury');
    expect(holderName('you (desktop)')).toBe('you');
    expect(holderName('client:Martin’s phone')).toBe('Martin’s phone');
  });

  it('says how long a claim has left', () => {
    expect(claimLeft(dev({}), 100)).toBeNull();
    expect(claimLeft(dev({ claimed_until: 100 }), 100)).toBe('lapsing');
    expect(claimLeft(dev({ claimed_until: 130 }), 100)).toBe('under a minute left');
    expect(claimLeft(dev({ claimed_until: 100 + 12 * 60 }), 100)).toBe('12 min left');
    expect(claimLeft(dev({ claimed_until: 100 + 5 * 3600 }), 100)).toBe('5 h left');
  });
});

describe('a debug_devices call', () => {
  it('reads its input', () => {
    const c = parseDeviceCall('{\n  "action": "run",\n  "device": "bench pixel",\n  "args": ["shell", "ls", 3]\n}');
    expect(c).toMatchObject({ action: 'run', device: 'bench pixel', args: ['shell', 'ls'], queue: false });
    expect(parseDeviceCall('not json')).toBeNull();
    expect(parseDeviceCall('[1]')).toBeNull();
  });

  it('says it in a line', () => {
    expect(deviceCallLine(parseDeviceCall('{"action":"screenshot","device":"pixel"}')!)).toBe('screenshot · pixel');
    expect(deviceCallLine(parseDeviceCall('{"action":"claim","device":"pixel","queue":true}')!)).toBe('wait in line · pixel');
    expect(deviceCallLine(parseDeviceCall('{"action":"boot","host":"mac","name":"Pixel_9"}')!)).toBe('boot · mac/Pixel_9');
    expect(deviceCallLine(parseDeviceCall('{"action":"list"}')!)).toBe('list');
  });

  it('reads its result by shape', () => {
    expect(parseDeviceResult(JSON.stringify(pixel))).toEqual({ kind: 'device', device: pixel });
    const list = parseDeviceResult(JSON.stringify({ devices: [pixel, { nope: 1 }], hosts: [] }));
    expect(list).toEqual({ kind: 'list', devices: [pixel] });
    expect(parseDeviceResult('{"exit_code":1,"output":"boom"}')).toEqual({
      kind: 'output',
      output: { exit_code: 1, output: 'boom', truncated: false },
    });
    expect(parseDeviceResult('Pixel 9 on mac: 3 bytes')).toEqual({ kind: 'text', text: 'Pixel 9 on mac: 3 bytes' });
    expect(parseDeviceResult('{"ok":true}')).toEqual({ kind: 'text', text: '{"ok":true}' });
  });
});
