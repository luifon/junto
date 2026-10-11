export function testEnvironment(caller) {
  return {
    JUNTO_TEST_LIVEKIT_CONTAINER: 'junto-dev-livekit-layout',
    JUNTO_TEST_LIVEKIT_PORT: '27880',
    JUNTO_TEST_LIVEKIT_TCP_PORT: '27881',
    JUNTO_TEST_LIVEKIT_UDP_PORT: '27882',
    JUNTO_TEST_WEBHOOK_ADDRESS: '127.0.0.1:27890',
    ...caller,
  };
}
