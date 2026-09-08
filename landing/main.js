const video = document.querySelector(".demo-video video");
const videoControl = document.querySelector(".video-control");

const updateVideoControl = () => {
  if (!(video instanceof HTMLVideoElement) || !(videoControl instanceof HTMLButtonElement)) return;
  const playing = !video.paused;
  videoControl.dataset.playing = String(playing);
  videoControl.setAttribute("aria-pressed", String(playing));
  videoControl.setAttribute("aria-label", playing ? "Pause product video" : "Play product video");
};

if (video instanceof HTMLVideoElement && videoControl instanceof HTMLButtonElement) {
  video.addEventListener("play", updateVideoControl);
  video.addEventListener("pause", updateVideoControl);
  video.addEventListener("ended", updateVideoControl);
  videoControl.addEventListener("click", () => {
    if (video.paused) {
      video.play().catch(updateVideoControl);
    } else {
      video.pause();
    }
  });
  updateVideoControl();
}
