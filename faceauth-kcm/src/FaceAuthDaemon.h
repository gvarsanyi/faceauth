// SPDX-License-Identifier: MIT OR Apache-2.0
#pragma once

#include <QDBusConnection>
#include <QDBusMessage>
#include <QObject>
#include <QVariantList>

#include <functional>

// Thin asynchronous client for the io.github.gvarsanyi.FaceAuth1 D-Bus API.
namespace FaceAuthDaemon {

inline constexpr const char *BUS_NAME = "io.github.gvarsanyi.FaceAuth1";
inline constexpr const char *OBJECT_PATH = "/io/github/gvarsanyi/FaceAuth1";
inline constexpr const char *INTERFACE = "io.github.gvarsanyi.FaceAuth1.Manager";

// Timeout for calls that do not open the camera or prompt for a password.
inline constexpr int QUICK_TIMEOUT_MS = 10000;
// Timeout for calls that may show a polkit prompt or run the camera.
inline constexpr int LONG_TIMEOUT_MS = 10 * 60 * 1000;

struct Result {
    bool ok = false;
    // True if the daemon could not be reached at all (not installed,
    // activation failed, bus timeout) rather than returning an error.
    bool unavailable = false;
    // D-Bus error name, e.g. io.github.gvarsanyi.FaceAuth1.Error.NoModel.
    QString errorName;
    QString errorMessage;
    QVariantList values;

    // True if errorName is io.github.gvarsanyi.FaceAuth1.Error.<shortName>.
    bool isError(const char *shortName) const;
};

using Callback = std::function<void(const Result &)>;

// The system bus, or the session bus when FACEAUTH_BUS=session (for a
// development daemon started with --session-bus).
QDBusConnection bus();

// Call a Manager method on bus(). With `interactive`, the call
// allows polkit to prompt for authentication. The callback runs on the
// event loop of `context`; it is dropped if `context` is destroyed first.
void call(const QString &method,
          const QVariantList &args,
          QObject *context,
          Callback callback,
          int timeoutMs = QUICK_TIMEOUT_MS,
          bool interactive = false);

} // namespace FaceAuthDaemon
