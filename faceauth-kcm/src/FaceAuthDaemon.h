#pragma once

#include <QJsonObject>
#include <QLocalSocket>
#include <QObject>
#include <QTimer>
#include <QVariant>
#include <functional>

// One DaemonRequest per daemon round-trip. Self-destructs via deleteLater()
// after the callback fires. Never reuse; create a fresh one per call.
class DaemonRequest : public QObject
{
    Q_OBJECT
public:
    // connectionError is true when the daemon could not be reached at all
    // (socket missing, connection refused, or timeout before any response).
    // It is false for protocol-level errors (daemon responded with "Err").
    using Callback = std::function<void(bool ok, QString error, QVariant data, bool connectionError)>;

    explicit DaemonRequest(QByteArray requestJson,
                           Callback callback,
                           int timeoutMs,
                           QObject *parent = nullptr);

private Q_SLOTS:
    void onConnected();
    void onReadyRead();
    void onError(QLocalSocket::LocalSocketError err);
    void onTimeout();

private:
    void finish(bool ok, const QString &error, const QVariant &data, bool connectionError = false);

    QLocalSocket *m_socket;
    QByteArray    m_requestJson;
    Callback      m_callback;
    QByteArray    m_buffer;
    bool          m_done = false;
};

// Public-facing factory namespace. All callers use FaceAuthDaemon::send().
namespace FaceAuthDaemon {

constexpr const char *SOCKET_PATH = "/run/faceauth/faceauth.sock";

// Default timeout for quick operations (LoadModel, SetConfig, Clear, Enroll).
constexpr int QUICK_TIMEOUT_MS = 8000;

// Fire a daemon request asynchronously. The callback is invoked on the Qt
// event loop thread once the daemon replies or the timeout expires.
//
// data in callback:
//   Ok       → {}
//   Err      → {}  (error string in the error parameter)
//   Model    → QString (raw model JSON)
//   Encoding → QVariantList of 128 doubles
//   Cameras  → QVariantList of QVariantMap {index, name, suitability}
//
// The DaemonRequest object is owned by `parent` during its lifetime and
// deletes itself after the callback returns.
void send(QJsonObject request,
          DaemonRequest::Callback callback,
          int timeoutMs,
          QObject *parent);

} // namespace FaceAuthDaemon
