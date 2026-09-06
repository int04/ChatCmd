import { describe, expect, it } from 'vitest';
import { webSocketAuthority } from '../realtime';

describe('local WebSocket authority', () => {
  it.each([
    ['http://localhost:8080/tasks/new', '127.0.0.1:8080'],
    ['http://localhost:5173/tasks/new', '127.0.0.1:5173'],
    ['http://localhost/tasks/new', '127.0.0.1'],
    ['http://127.0.0.1:18080/tasks/new', '127.0.0.1:18080'],
    ['http://[::1]:8080/tasks/new', '[::1]:8080'],
    ['https://localhost:8443/tasks/new', 'localhost:8443'],
    ['https://console.example.test/tasks/new', 'console.example.test'],
  ])('selects the expected authority for %s', (href, expected) => {
    expect(webSocketAuthority(new URL(href))).toBe(expected);
  });
});
