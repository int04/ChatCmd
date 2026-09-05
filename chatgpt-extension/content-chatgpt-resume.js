// Restore only the exact request/user checkpoint. Never resubmit a prompt after reload.
(() => {
  const controller = globalThis.ChatCmdController;
  async function resume() {
    if (!controller?.current() || controller.active) return;
    try {
      const response = await globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-chatgpt-observation-resume' });
      if (!response?.ok || !response.request || !controller.current() || controller.active) return;
      const checkpoint = globalThis.ChatCmdObserver.restore(response.request.id);
      if (!checkpoint?.userId || checkpoint.conversationId !== globalThis.ChatCmdTranscript.conversationId()) return;
      void controller.adopt(response.request);
    } catch (error) { globalThis.ChatCmdCaptureStatus?.report('error', String(error?.message || error)); }
  }
  globalThis.ChatCmdResumeReady = resume();
})();
