// 16 kHz mono PCM16 capture for the Stark live voice session.
// The browser is the only microphone owner: AEC/NS/AGC happen here, before
// the audio reaches the Rust STT worker over the voice WebSocket.

class StarkPcmProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.ratio = sampleRate / 16000;
    this.pending = new Float32Array(0);
    this.readAt = 0;
    this.frame = new Int16Array(320);
    this.frameAt = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || !input.length) return true;
    const frames = input[0].length;
    const channels = input.length;
    if (!frames) return true;

    const mono = new Float32Array(frames);
    for (let channel = 0; channel < channels; channel++) {
      const data = input[channel];
      for (let i = 0; i < frames; i++) mono[i] += data[i] / channels;
    }

    // Append to a small resampling buffer. Keeping the buffer across blocks
    // preserves fractional positions when the device rate is not a multiple
    // of 16 kHz (e.g. 24 kHz Pocket TTS context or 44.1 kHz devices).
    const buffer = new Float32Array(this.pending.length + frames);
    buffer.set(this.pending);
    buffer.set(mono, this.pending.length);
    while (this.readAt + 1 < buffer.length) {
      const index = Math.floor(this.readAt);
      const fraction = this.readAt - index;
      const sample = buffer[index] * (1 - fraction) + buffer[index + 1] * fraction;
      const clamped = sample > 1 ? 1 : sample < -1 ? -1 : sample;
      this.frame[this.frameAt++] = clamped < 0 ? clamped * 32768 : clamped * 32767;
      if (this.frameAt === this.frame.length) {
        const copy = this.frame.slice();
        this.port.postMessage(copy.buffer, [copy.buffer]);
        this.frameAt = 0;
      }
      this.readAt += this.ratio;
    }
    const consumed = Math.floor(this.readAt);
    if (consumed > 0) {
      this.pending = buffer.slice(Math.min(consumed, buffer.length));
      this.readAt -= consumed;
    } else {
      this.pending = buffer;
    }
    return true;
  }
}

registerProcessor('stark-pcm', StarkPcmProcessor);
