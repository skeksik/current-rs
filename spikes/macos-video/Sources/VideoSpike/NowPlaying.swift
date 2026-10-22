import AVFoundation
import MediaPlayer

/// MPNowPlayingInfoCenter and MPRemoteCommandCenter driven by our own player (no AVPlayerViewController).
@MainActor
final class NowPlaying {
    static let shared = NowPlaying()
    private weak var model: PlayerModel?

    func attach(to model: PlayerModel) {
        self.model = model
        let center = MPRemoteCommandCenter.shared()
        on(center.playCommand) { $0.player.play() }
        on(center.pauseCommand) { $0.player.pause() }
        on(center.togglePlayPauseCommand) { $0.togglePlay() }
        on(center.nextTrackCommand) { $0.next() }
        on(center.previousTrackCommand) { $0.previous() }
        center.skipForwardCommand.preferredIntervals = [10]
        center.skipBackwardCommand.preferredIntervals = [10]
        on(center.skipForwardCommand) { $0.skip(10) }
        on(center.skipBackwardCommand) { $0.skip(-10) }
        center.changePlaybackPositionCommand.addTarget { event in
            let position = (event as? MPChangePlaybackPositionCommandEvent)?.positionTime ?? 0
            let main = Thread.isMainThread
            Task { @MainActor in
                NowPlaying.shared.model?.note("remote: changePlaybackPosition \(Int(position)) s (main thread: \(main))")
                NowPlaying.shared.model?.seek(to: position)
            }
            return .success
        }
    }

    private func on(_ command: MPRemoteCommand, _ action: @escaping @MainActor (PlayerModel) -> Void) {
        command.isEnabled = true
        command.addTarget { event in
            let name = String(describing: type(of: event.command))
            let main = Thread.isMainThread
            Task { @MainActor in
                guard let model = NowPlaying.shared.model else { return }
                model.note("remote: \(name) (main thread: \(main))")
                action(model)
            }
            return .success
        }
    }

    /// Called on item, rate and position changes only; the system extrapolates elapsed time from the rate.
    func update(from model: PlayerModel) {
        let info = MPNowPlayingInfoCenter.default()
        guard model.player.currentItem != nil else {
            info.nowPlayingInfo = nil
            info.playbackState = .stopped
            return
        }
        info.nowPlayingInfo = [
            MPMediaItemPropertyTitle: model.title,
            MPMediaItemPropertyPlaybackDuration: model.duration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: model.player.currentTime().seconds.isFinite
                ? model.player.currentTime().seconds : 0,
            MPNowPlayingInfoPropertyPlaybackRate: model.isPlaying ? Double(model.player.rate) : 0,
            MPNowPlayingInfoPropertyDefaultPlaybackRate: 1.0,
            MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.video.rawValue,
            MPNowPlayingInfoPropertyPlaybackQueueCount: model.queue.count,
        ]
        info.playbackState = model.isPlaying ? .playing : .paused
    }
}
