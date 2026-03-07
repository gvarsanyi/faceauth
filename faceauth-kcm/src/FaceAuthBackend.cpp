#include "FaceAuthBackend.h"
#include "FaceAuthDaemon.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QProcess>

#include <pwd.h>
#include <unistd.h>

// --- Daemon availability ---

void FaceAuthBackend::checkAvailability(bool ok, bool connectionError)
{
    const bool wasAvailable = m_daemonAvailable;
    if (ok) {
        m_daemonAvailable = true;
    } else if (connectionError) {
        m_daemonAvailable = false;
    }
    if (m_daemonAvailable == wasAvailable)
        return;
    Q_EMIT daemonAvailableChanged();
    if (m_daemonAvailable) {
        listCameras();   // reload camera list after down->up recovery
        refresh();       // reload model state after down->up recovery
        getServices();   // reload services list after down->up recovery
    }
}

// --- Camera enumeration ---

void FaceAuthBackend::pingDaemon()
{
    // Health-check only: uses ListCameras as a lightweight probe but does not
    // update m_cameras. Camera data is loaded at startup and on recovery.
    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"), QLatin1String("ListCameras")}},
        [this](bool ok, const QString &, const QVariant &, bool connectionError) {
            checkAvailability(ok, connectionError);
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

void FaceAuthBackend::listCameras()
{
    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"), QLatin1String("ListCameras")}},
        [this](bool ok, const QString &, const QVariant &data, bool connectionError) {
            checkAvailability(ok, connectionError);
            if (!ok)
                return;
            m_cameras = data.toList();
            Q_EMIT camerasChanged();
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

// --- Constructor ---

FaceAuthBackend::FaceAuthBackend(QObject *parent)
    : QObject(parent)
{
    // Resolve the current user once at startup.
    const uid_t uid = ::getuid();
    m_uid = static_cast<quint32>(uid);
    const struct passwd *pw = ::getpwuid(uid);
    m_username = pw ? QString::fromLocal8Bit(pw->pw_name) : QString();

    // Health-check the daemon every 2 s. Detects both the daemon going down
    // mid-session (up->down) and recovery (down->up). Always running.
    m_pollTimer = new QTimer(this);
    m_pollTimer->setInterval(2000);
    m_pollTimer->setSingleShot(false);
    connect(m_pollTimer, &QTimer::timeout, this, &FaceAuthBackend::pingDaemon);
    m_pollTimer->start();

    listCameras();
    refresh();
    getServices();
}

// --- Private helpers ---

void FaceAuthBackend::setBusy(bool b)
{
    if (m_busy == b)
        return;
    m_busy = b;
    Q_EMIT busyChanged();
}

void FaceAuthBackend::setLastError(const QString &err)
{
    if (m_lastError == err)
        return;
    m_lastError = err;
    Q_EMIT lastErrorChanged();
}

void FaceAuthBackend::parseModelJson(const QString &rawJson)
{
    const QJsonDocument doc = QJsonDocument::fromJson(rawJson.toUtf8());
    const QJsonObject root = doc.object();

    m_batchCount = root.value(QLatin1String("encodings")).toArray().size();

    const QJsonObject cam = root.value(QLatin1String("camera")).toObject();
    m_cameraIndex = cam.value(QLatin1String("index")).toInt();

    Q_EMIT modelChanged();
}

// --- sendAndRefresh helper ---

void FaceAuthBackend::sendAndRefresh(QJsonObject request)
{
    setBusy(true);
    FaceAuthDaemon::send(
        std::move(request),
        [this](bool ok, const QString &err, const QVariant &, bool connectionError) {
            setBusy(false);
            checkAvailability(ok, connectionError);
            if (ok)
                refresh();
            else if (!connectionError)
                setLastError(err);
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

// --- Public slots ---

void FaceAuthBackend::refresh()
{
    if (m_username.isEmpty())
        return;

    setBusy(true);
    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"), QLatin1String("LoadModel")},
                    {QLatin1String("username"), m_username}},
        [this](bool ok, const QString &err, const QVariant &data, bool connectionError) {
            setBusy(false);
            checkAvailability(ok, connectionError);
            if (ok) {
                parseModelJson(data.toString());
            } else {
                // Connection errors are shown via the daemon-unavailable banner;
                // "no model enrolled" is not an error either.
                if (!connectionError && !err.contains(QLatin1String("no model enrolled"))) {
                    setLastError(err);
                }
                // Reset to unenrolled state.
                m_batchCount = 0;
                Q_EMIT modelChanged();
            }
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

void FaceAuthBackend::setServiceOpt(const QString &service, bool allowed)
{
    if (service.trimmed().isEmpty())
        return;
    setBusy(true);
    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"),      QLatin1String("SetOpt")},
                    {QLatin1String("username"), m_username},
                    {QLatin1String("service"),  service.trimmed()},
                    {QLatin1String("allowed"),  allowed}},
        [this](bool ok, const QString &err, const QVariant &, bool connectionError) {
            setBusy(false);
            checkAvailability(ok, connectionError);
            if (ok)
                getServices();
            else if (!connectionError)
                setLastError(err);
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

void FaceAuthBackend::getServices()
{
    if (m_username.isEmpty())
        return;

    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"),      QLatin1String("GetServices")},
                    {QLatin1String("username"), m_username}},
        [this](bool ok, const QString &, const QVariant &data, bool connectionError) {
            checkAvailability(ok, connectionError);
            if (!ok)
                return;
            m_servicesList = data.toList();
            Q_EMIT servicesChanged();
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

void FaceAuthBackend::clearAll()
{
    // index: null removes the entire model.
    sendAndRefresh(QJsonObject{{QLatin1String("op"), QLatin1String("Clear")},
                               {QLatin1String("username"), m_username},
                               {QLatin1String("index"), QJsonValue::Null}});
}

// --- Desktop notification helpers ---

static QString notifyBin()
{
    return QStringLiteral(FACEAUTH_LIBEXEC_DIR "/faceauth-notify");
}

// Spawn `faceauth-notify start <uid> faceauth` asynchronously. The notification
// ID is stored in m_notifId when the helper exits; the daemon call runs in
// parallel. Since the helper only makes a single D-Bus call, it reliably
// finishes well before the 10-second authenticate timeout.
void FaceAuthBackend::notifyStart()
{
    m_notifId = 0;
    auto *proc = new QProcess(this);
    connect(proc, &QProcess::finished, this,
            [this, proc](int exitCode, QProcess::ExitStatus) {
                if (exitCode == 0) {
                    bool ok;
                    quint32 id = proc->readAllStandardOutput().trimmed().toUInt(&ok);
                    if (ok)
                        m_notifId = id;
                }
                proc->deleteLater();
            });
    proc->start(notifyBin(), {QStringLiteral("start"),
                               QString::number(m_uid),
                               QStringLiteral("faceauth")});
}

// Spawn `faceauth-notify success|failure <uid> <notifId> faceauth` to replace
// the in-progress notification with the final result. Fire-and-forget.
void FaceAuthBackend::notifyFinish(quint32 notifId, bool success)
{
    if (notifId == 0)
        return;
    const QString sub = success ? QStringLiteral("success") : QStringLiteral("failure");
    QProcess::startDetached(notifyBin(), {sub,
                                          QString::number(m_uid),
                                          QString::number(notifId),
                                          QStringLiteral("faceauth")});
}

// --- Test authentication ---

void FaceAuthBackend::testAuth()
{
    if (m_testing || m_busy)
        return;

    static constexpr int TEST_TIMEOUT_SECS = 5; // matches CLI default and PAM module

    m_testing = true;
    Q_EMIT testingChanged();
    setBusy(true);

    // Fire the "authenticating..." desktop notification asynchronously.
    // The daemon call runs in parallel; m_notifId is populated by the time
    // the authenticate response arrives (the helper is far faster than 5 s).
    notifyStart();

    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"),           QLatin1String("Authenticate")},
                    {QLatin1String("username"),     m_username},
                    {QLatin1String("timeout_secs"), TEST_TIMEOUT_SECS}},
        [this](bool ok, const QString &err, const QVariant &, bool connectionError) {
            setBusy(false);
            checkAvailability(ok, connectionError);
            m_testing = false;
            Q_EMIT testingChanged();
            notifyFinish(m_notifId, ok);
            m_notifId = 0;
            if (ok) {
                Q_EMIT authSucceeded();
            } else {
                // Always emit so QML can reset testMode.
                // Connection errors are shown via the daemon-unavailable banner.
                Q_EMIT authFailed(connectionError ? QString() : err);
            }
        },
        (TEST_TIMEOUT_SECS + 10) * 1000,
        this);
}

// --- Enrollment state machine ---

void FaceAuthBackend::startEnrollment(int cameraIndex)
{
    if (m_enrolling || m_busy)
        return;

    m_cameraIndex = cameraIndex;
    m_capturesDone = 0;
    m_capturedEncodings.clear();
    m_enrolling = true;
    Q_EMIT enrollingChanged();
    Q_EMIT capturesDoneChanged();

    captureOne();
}

void FaceAuthBackend::cancelEnrollment()
{
    if (!m_enrolling)
        return;
    m_enrolling = false;
    m_capturesDone = 0;
    m_capturedEncodings.clear();
    setBusy(false);
    Q_EMIT enrollingChanged();
    Q_EMIT capturesDoneChanged();
}

void FaceAuthBackend::captureOne()
{
    setBusy(true);
    // Per-capture timeout: 30 s for the camera + 10 s headroom in the socket layer.
    const int timeoutMs = (CAPTURE_TIMEOUT_SECS + 10) * 1000;

    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"), QLatin1String("CaptureEncoding")},
                    {QLatin1String("camera_index"), m_cameraIndex},
                    {QLatin1String("timeout_secs"), CAPTURE_TIMEOUT_SECS}},
        [this](bool ok, const QString &err, const QVariant &data, bool connectionError) {
            setBusy(false);
            checkAvailability(ok, connectionError);
            if (!m_enrolling)
                return; // cancelled while waiting

            if (!ok) {
                if (!connectionError)
                    setLastError(err);
                Q_EMIT operationFailed(err);
                cancelEnrollment();
                return;
            }

            m_capturedEncodings.append(data.toList());
            ++m_capturesDone;
            Q_EMIT capturesDoneChanged();

            if (m_capturesDone < CAPTURES_REQUIRED) {
                captureOne(); // next capture
            } else {
                submitEnrollment();
            }
        },
        timeoutMs,
        this);
}

void FaceAuthBackend::submitEnrollment()
{
    // Build the encodings array: each capture is an array of 128 floats.
    QJsonArray encodingsArray;
    for (const QVariantList &capture : std::as_const(m_capturedEncodings)) {
        QJsonArray captureArray;
        for (const QVariant &v : capture)
            captureArray.append(v.toDouble());
        encodingsArray.append(captureArray);
    }

    setBusy(true);
    // Enroll can take a moment (disk write); use the quick timeout.
    FaceAuthDaemon::send(
        QJsonObject{{QLatin1String("op"), QLatin1String("Enroll")},
                    {QLatin1String("username"), m_username},
                    {QLatin1String("camera_index"), m_cameraIndex},
                    {QLatin1String("encodings"), encodingsArray}},
        [this](bool ok, const QString &err, const QVariant &, bool connectionError) {
            setBusy(false);
            checkAvailability(ok, connectionError);
            m_enrolling = false;
            m_capturesDone = 0;
            m_capturedEncodings.clear();
            Q_EMIT enrollingChanged();
            Q_EMIT capturesDoneChanged();

            if (ok) {
                refresh();
                Q_EMIT enrollmentSucceeded();
            } else {
                if (!connectionError)
                    setLastError(err);
                Q_EMIT operationFailed(err);
            }
        },
        FaceAuthDaemon::QUICK_TIMEOUT_MS,
        this);
}

void FaceAuthBackend::clearLastError()
{
    setLastError(QString());
}
