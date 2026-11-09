//! Control smart electronic toys created by DG-LAB from the CLI.

use std::{
    convert::Infallible, ops::Range, pin::pin, str::FromStr, sync::LazyLock, time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Parser, Subcommand};
use color_eyre::eyre::{self, OptionExt};
use dungeonctl::{
    audio::{AudioStream, pipewire::pipewire_sink, symphonia::audio_file},
    core::StereoValue,
    device::{
        Coyote3,
        coyote3::{DeviceSettings, IntensityChange, Pulse, Pulses},
    },
    graph::PatternToSvg,
    pattern::{
        AudioPatternRange, Mono, MonoPattern, Relative, Stereo, StereoFrame, StereoPattern,
        stream::{MonoPatternStreamExt, PatternStream, PatternStreamExt, StereoPatternStreamExt},
    },
    utils::futures_signals::signal::SignalExt,
};
use either::Either;
use futures::{Stream, StreamExt, TryStreamExt, stream::LocalBoxStream};
use indicatif::ProgressStyle;
use tracing::{debug, info, trace_span};
use tracing_indicatif::{IndicatifLayer, filter::IndicatifFilter, span_ext::IndicatifSpanExt};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

static PROGRESS_STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
    ProgressStyle::with_template("{msg}  {wide_bar:.green/black}  {percent} %  {eta_precise:.cyan}")
        .unwrap()
        .progress_chars("━━━")
});

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Play an audio file or a .hwl file
    Play {
        /// Path to an audio or .hwl pattern file
        path: Utf8PathBuf,

        #[command(flatten)]
        options: PlaybackOptions,

        #[arg(long, default_value = "10", value_parser = duration_secs)]
        ramp: Duration,
    },
    /// Capture audio from your system
    Capture {
        #[command(flatten)]
        options: PlaybackOptions,

        #[arg(long, default_value = "200.0-1000.0", value_parser = frequency_range::<f32>)]
        input_frequency_range: StereoValue<Range<f32>>,

        #[arg(long)]
        mono: bool,
    },
    /// Convert an audio file into a .hwl file
    Convert {
        /// Audio file
        input: Utf8PathBuf,
        /// .hwl file
        output: Utf8PathBuf,
    },
    /// Render an audio file as a SVG graph
    Graph {
        /// Audio file
        input: Utf8PathBuf,
        /// .svg file
        output: Utf8PathBuf,
        /// Affects the image size and resolution, higher value means that more frames are skipped
        #[arg(long, default_value = "1")]
        step: usize,
    },
}

#[derive(Debug, Args)]
struct PlaybackOptions {
    #[arg(long, default_value = "70,70", value_parser = stereo_value::<u8>)]
    limit: StereoValue<u8>,

    #[arg(long, default_value = "160,160", value_parser = stereo_value::<u8>)]
    frequency_balance: StereoValue<u8>,

    #[arg(long, default_value = "0,0", value_parser = stereo_value::<u8>)]
    intensity_balance: StereoValue<u8>,

    #[arg(long, default_value = "20-100", value_parser = frequency_range::<u8>)]
    frequency_range: StereoValue<Range<u8>>,

    #[arg(long, default_value = "50,50", value_parser = stereo_value::<u8>)]
    intensity: StereoValue<u8>,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let indicatif_layer = IndicatifLayer::new();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_file(true)
                .with_line_number(true)
                .with_writer(indicatif_layer.get_stdout_writer())
                .with_filter(tracing_subscriber::EnvFilter::try_from_default_env().or_else(|_| {
                    "info,dungeonctl=debug,dungeonctl_audio=debug,dungeonctl_cli=debug,dungeonctl_core=debug,dungeonctl_device=info".parse()
                })?),
        )
        .with(indicatif_layer.with_filter(IndicatifFilter::new(false)))
        .init();

    _main().await
}

async fn _main() -> color_eyre::Result<()> {
    let cli = Cli::parse();
    debug!(?cli);

    match &cli.command {
        Commands::Play {
            path,
            options:
                PlaybackOptions {
                    limit,
                    frequency_balance,
                    intensity_balance,
                    frequency_range,
                    intensity,
                },
            ramp,
        } => {
            let stream: Relative<LocalBoxStream<Result<StereoFrame, Infallible>>> =
                match load_pattern(path).await? {
                    Either::Left(Either::Left(pattern)) => pattern
                        .into_relative()
                        .map(|p| p.into_stream().into_stereo().boxed_local()),
                    Either::Left(Either::Right(pattern)) => pattern
                        .into_relative()
                        .map(|p| p.into_stream().boxed_local()),
                    Either::Right(pattern) => pattern.map(|p| p.into_stream().boxed_local()),
                };

            let coyote = Coyote3::connect()
                .settings(DeviceSettings {
                    limit: *limit,
                    frequency_balance: *frequency_balance,
                    intensity_balance: *intensity_balance,
                })
                .await?;

            tokio::select!(
                _ = coyote.state().for_each(async |state| info!(?state)) => Ok(()),
                res = async {
                    play_pattern(&coyote, stream, frequency_range.clone(), *intensity, Some(*ramp)).await?;

                    coyote.disconnect().await?;

                    eyre::Ok(())
                } => res,
                res = async {
                    tokio::signal::ctrl_c()
                        .await?;

                    coyote.disconnect().await?;

                    eyre::Ok(())
                } => res,
            )?;
        }
        Commands::Capture {
            options:
                PlaybackOptions {
                    limit,
                    frequency_balance,
                    intensity_balance,
                    frequency_range,
                    intensity,
                },
            input_frequency_range,
            mono,
        } => {
            let stream: Relative<LocalBoxStream<eyre::Result<StereoFrame>>> = if *mono {
                pipewire_sink::<Mono>()
                    .audio_pattern()
                    .into_relative(AudioPatternRange {
                        amplitude: 0.0..1.0,
                        frequency: input_frequency_range.a.clone(),
                    })
                    .map(|stream| stream.into_stereo().boxed_local())
            } else {
                pipewire_sink::<Stereo>()
                    .audio_pattern()
                    .into_relative(StereoValue {
                        a: AudioPatternRange {
                            amplitude: 0.0..1.0,
                            frequency: input_frequency_range.a.clone(),
                        },
                        b: AudioPatternRange {
                            amplitude: 0.0..1.0,
                            frequency: input_frequency_range.b.clone(),
                        },
                    })
                    .map(|stream| stream.boxed_local())
            };

            let coyote = Coyote3::connect()
                .settings(DeviceSettings {
                    limit: *limit,
                    frequency_balance: *frequency_balance,
                    intensity_balance: *intensity_balance,
                })
                .await?;

            tokio::select!(
                _ = coyote.state().for_each(async |state| info!(?state)) => Ok(()),
                res = async {
                    play_pattern(&coyote, stream, frequency_range.clone(), *intensity, None).await?;

                    coyote.disconnect().await?;

                    eyre::Ok(())
                } => res,
                res = async {
                    tokio::signal::ctrl_c()
                        .await?;

                    coyote.disconnect().await?;

                    eyre::Ok(())
                } => res,
            )?;
        }
        Commands::Convert { input, output } => {
            let pattern = load_pattern(input)
                .await?
                .right_or_else(|p| p.right_or_else(|p| p.to_stereo()).into_relative());

            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }

            info!("writing audio pattern to '{output}'");
            pattern.write_hwl_file(output)?;
        }
        Commands::Graph {
            input,
            output,
            step,
        } => {
            if *step == 0 {
                eyre::bail!("step size must be >= 1")
            }

            let svg = match load_pattern(input).await? {
                Either::Left(pattern) => pattern
                    .map_either(|p| p.to_svg(*step), |p| p.to_svg(*step))
                    .either_into(),
                Either::Right(pattern) => pattern.to_svg(*step),
            };

            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }

            info!("writing graph to '{output}'");
            std::fs::write(output, svg.to_string())?;
        }
    };

    Ok(())
}

async fn load_pattern(
    path: &Utf8Path,
) -> eyre::Result<Either<Either<MonoPattern, StereoPattern>, Relative<StereoPattern>>> {
    let pattern = if path.extension() == Some("hwl") {
        Either::Right(Relative::<StereoPattern>::from_hwl_file(path)?)
    } else {
        info!("converting audio file to pattern, this may take a while");

        let stream = audio_file(path.to_path_buf().into_std_path_buf())
            .await?
            .map_either(AudioStream::audio_pattern, AudioStream::audio_pattern);

        let progress = {
            let progress = trace_span!("converting", indicatif.pb_show = tracing::field::Empty);
            let duration = match &stream {
                Either::Left(s) => s.size_hint().1,
                Either::Right(s) => s.size_hint().1,
            };
            progress.pb_set_style(&PROGRESS_STYLE);
            if let Some(duration) = duration {
                progress.pb_set_length(duration as u64);
            }
            progress.pb_set_message("Converting audio file to pattern");
            progress.entered()
        };

        Either::Left(match stream {
            Either::Left(s) => {
                Either::Left(MonoPattern::from_stream(s.inspect_ok(|_| progress.pb_inc(1))).await?)
            }
            Either::Right(s) => Either::Right(
                StereoPattern::from_stream(s.inspect_ok(|_| progress.pb_inc(1))).await?,
            ),
        })
    };

    Ok(pattern)
}

async fn play_pattern<E>(
    coyote: &Coyote3,
    Relative(pattern): Relative<impl PatternStream<Stereo, Error = E> + Sized + Unpin>,
    frequency_range: StereoValue<Range<u8>>,
    intensity: StereoValue<u8>,
    ramp: Option<Duration>,
) -> eyre::Result<()>
where
    eyre::Report: From<E>,
{
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let ramp_steps = ramp.map(|ramp| ramp.as_secs() * 10);

    let chunks = pin!(pattern.const_chunks::<4>());
    let mut enumerated_chunks = chunks.enumerate();

    while let Some((i, result)) = enumerated_chunks.next().await {
        let chunk: [StereoFrame; 4] = result?;

        let intensity = match ramp_steps {
            Some(ramp_steps) if (i as u64) < ramp_steps => StereoValue {
                a: IntensityChange::AbsoluteChange(
                    (intensity.a as f32 * (i as f32 / ramp_steps as f32)).round() as u8,
                ),
                b: IntensityChange::AbsoluteChange(
                    (intensity.b as f32 * (i as f32 / ramp_steps as f32)).round() as u8,
                ),
            },
            None if i == 0 => StereoValue {
                a: IntensityChange::AbsoluteChange(intensity.a),
                b: IntensityChange::AbsoluteChange(intensity.b),
            },
            _ => StereoValue::symmetric(IntensityChange::DoNotChange),
        };

        coyote
            .send_pulses(Pulses {
                intensity,
                pulses: chunk.map(|frame| StereoValue {
                    a: Pulse {
                        frequency: (frame.a.frequency
                            * (frequency_range.a.end - frequency_range.a.start) as f32
                            + frequency_range.a.start as f32)
                            as u8,
                        intensity: (frame.a.amplitude * 100.0) as u8,
                    },
                    b: Pulse {
                        frequency: (frame.b.frequency
                            * (frequency_range.b.end - frequency_range.b.start) as f32
                            + frequency_range.b.start as f32)
                            as u8,
                        intensity: (frame.b.amplitude * 100.0) as u8,
                    },
                }),
            })
            .await?;

        interval.tick().await;
    }

    Ok(())
}

fn stereo_value<T>(s: &str) -> eyre::Result<StereoValue<T>>
where
    T: FromStr + Copy,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let parts = s
        .split(',')
        .map(T::from_str)
        .collect::<Result<Vec<T>, T::Err>>()?;

    match &parts[..] {
        [v] => Ok(StereoValue::symmetric(*v)),
        [a, b] => Ok(StereoValue { a: *a, b: *b }),
        _ => eyre::bail!("received more than 2 args: {s}"),
    }
}

fn frequency_range<T: Clone + FromStr<Err: std::error::Error + Send + Sync + 'static>>(
    s: &str,
) -> eyre::Result<StereoValue<Range<T>>> {
    let parts = s
        .split(',')
        .map(|s| {
            s.split_once('-')
                .map(|(min, max)| eyre::Ok((min.parse::<T>()?, max.parse::<T>()?)))
                .transpose()?
                .ok_or_eyre("mising '-'")
                .map(|(min, max)| min..max)
        })
        .collect::<eyre::Result<Vec<_>>>()?;

    match &parts[..] {
        [v] => Ok(StereoValue {
            a: v.clone(),
            b: v.clone(),
        }),
        [a, b] => Ok(StereoValue {
            a: a.clone(),
            b: b.clone(),
        }),
        _ => eyre::bail!("received more than 2 args: {s}"),
    }
}

fn duration_secs(s: &str) -> eyre::Result<Duration> {
    Ok(Duration::from_secs(s.parse()?))
}
