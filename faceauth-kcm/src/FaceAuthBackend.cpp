// SPDX-License-Identifier: MIT OR Apache-2.0
#include "FaceAuthBackend.h"
#include "FaceAuthDaemon.h"

#include <QDBusArgument>
#include <QDBusServiceWatcher>
#include <QTimer>

#include <pwd.h>
#include <unistd.h>

using FaceAuthDaemon::Result;

namespace {

// Seconds the test dialog waits; matches the daemon default and the QML bar.
constexpr int TEST_TIMEOUT_SECS = 5;

// Sentinel camera index: reuse the camera of the existing model.
constexpr int ENROLLED_CAMERA = -1;

// Decode ListCameras' a(usy) reply.
QVariantList decodeCameras(const QVariant &value)
{
    QVariantList cameras;
    const QDBusArgument arg = value.value<QDBusArgument>();
    arg.beginArray();
    while (!arg.atEnd()) {
        uint index = 0;
        QString name;
        uchar suitability = 0;
        arg.beginStructure();
        arg >> index >> name >> suitability;
        arg.endStructure();
        const QString label = name.isEmpty() ? QStringLiteral("/dev/video%1").arg(index)
                                             : QStringLiteral("/dev/video%1 %2").arg(index).arg(name);
        cameras.append(QVariantMap{{QStringLiteral("index"), int(index)},
                                   {QStringLiteral("name"), label},
                                   {QStringLiteral("suitability"), int(suitability)}});
    }
    arg.endArray();
    return cameras;
}

// Decode GetServices' a(sb) reply.
QVariantList decodeServices(const QVariant &value)
{
    QVariantList services;
    const QDBusArgument arg = value.value<QDBusArgument>();
    arg.beginArray();
    while (!arg.atEnd()) {
        QString name;
        bool enabled = false;
        arg.beginStructure();
        arg >> name >> enabled;
        arg.endStructure();
        services.append(QVariantMap{{QStringLiteral("name"), name}, {QStringLiteral("enabled"), enabled}});
    }
    arg.endArray();
    return services;
}

} // namespace

FaceAuthBackend::FaceAuthBackend(QObject *parent)
    : QObject(parent)
{
    const struct passwd *pw = ::getpwuid(::getuid());
    m_username = pw ? QString::fromLocal8Bit(pw->pw_name) : QString();

    QDBusConnection bus = FaceAuthDaemon::bus();
    bus.connect(QLatin1String(FaceAuthDaemon::BUS_NAME),
                QLatin1String(FaceAuthDaemon::OBJECT_PATH),
                QLatin1String(FaceAuthDaemon::INTERFACE),
                QStringLiteral("EnrollProgress"),
                this,
                SLOT(onEnrollProgress(uint, uint, QString)));

    // The daemon is D-Bus activated, so its name is only owned while it runs.
    // Reload when it appears (e.g. started by another client after it was
    // unavailable), and shortly after it goes away (crash, restart).
    auto *watcher = new QDBusServiceWatcher(QLatin1String(FaceAuthDaemon::BUS_NAME), bus,
                                            QDBusServiceWatcher::WatchForOwnerChange, this);
    connect(watcher, &QDBusServiceWatcher::serviceRegistered, this, &FaceAuthBackend::refresh);
    connect(watcher, &QDBusServiceWatcher::serviceUnregistered, this, &FaceAuthBackend::onDaemonUnregistered);

    refresh();
}

// --- Helpers ---

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

void FaceAuthBackend::setEnrolling(bool e)
{
    if (m_enrolling == e)
        return;
    m_enrolling = e;
    Q_EMIT enrollingChanged();
}

bool FaceAuthBackend::track(const Result &r)
{
    const bool available = !r.unavailable;
    if (available != m_daemonAvailable) {
        m_daemonAvailable = available;
        Q_EMIT daemonAvailableChanged();
    }
    return !r.ok && available;
}

void FaceAuthBackend::reportFailure(const Result &r)
{
    if (!track(r))
        return;
    // A dismissed password prompt is not an error worth a banner.
    if (r.isError("NotAuthorized"))
        return;
    setLastError(r.errorMessage);
}

void FaceAuthBackend::onDaemonUnregistered()
{
    QTimer::singleShot(1000, this, &FaceAuthBackend::refresh);
}

// --- Loading state ---

void FaceAuthBackend::refresh()
{
    loadCameras();
    loadModel();
    loadServices();
}

void FaceAuthBackend::loadCameras()
{
    FaceAuthDaemon::call(QStringLiteral("ListCameras"), {}, this, [this](const Result &r) {
        if (track(r) || !r.ok)
            return;
        m_cameras = decodeCameras(r.values.value(0));
        Q_EMIT camerasChanged();
    });
}

void FaceAuthBackend::loadModel()
{
    if (m_username.isEmpty())
        return;
    FaceAuthDaemon::call(QStringLiteral("GetModel"), {m_username}, this, [this](const Result &r) {
        const bool daemonError = track(r);
        if (r.ok) {
            m_cameraIndex = r.values.value(1).toInt();
            m_batchCount = int(r.values.value(2).toUInt());
        } else {
            m_cameraIndex = -1;
            m_batchCount = 0;
            if (daemonError && !r.isError("NoModel"))
                setLastError(r.errorMessage);
        }
        Q_EMIT modelChanged();
    });
}

void FaceAuthBackend::loadServices()
{
    if (m_username.isEmpty())
        return;
    FaceAuthDaemon::call(QStringLiteral("GetServices"), {m_username}, this, [this](const Result &r) {
        if (track(r) || !r.ok)
            return;
        m_servicesList = decodeServices(r.values.value(0));
        Q_EMIT servicesChanged();
    });
}

// --- Actions ---

void FaceAuthBackend::setServiceEnabled(const QString &service, bool enabled)
{
    setBusy(true);
    FaceAuthDaemon::call(
        QStringLiteral("SetServiceEnabled"), {m_username, service, enabled}, this,
        [this](const Result &r) {
            setBusy(false);
            reportFailure(r);
            // Reload either way so the checkbox reflects the stored state.
            loadServices();
        },
        FaceAuthDaemon::LONG_TIMEOUT_MS, true);
}

void FaceAuthBackend::clearAll()
{
    setBusy(true);
    FaceAuthDaemon::call(
        QStringLiteral("DeleteModel"), {m_username}, this,
        [this](const Result &r) {
            setBusy(false);
            reportFailure(r);
            loadModel();
        },
        FaceAuthDaemon::LONG_TIMEOUT_MS, true);
}

void FaceAuthBackend::startEnrollment(int cameraIndex)
{
    if (m_enrolling || m_busy)
        return;
    setLastError(QString());
    m_capturesDone = 0;
    Q_EMIT capturesDoneChanged();
    setEnrolling(true);
    setBusy(true);

    const int index = enrolled() ? ENROLLED_CAMERA : cameraIndex;
    FaceAuthDaemon::call(
        QStringLiteral("Enroll"), {m_username, index}, this,
        [this](const Result &r) {
            setBusy(false);
            // Report the outcome before enrolling turns false: the dialog
            // closes itself on a false enrolling without success or error.
            if (r.ok) {
                loadModel();
                Q_EMIT enrollmentSucceeded();
            } else if (!r.isError("Cancelled")) {
                reportFailure(r);
            }
            setEnrolling(false);
        },
        FaceAuthDaemon::LONG_TIMEOUT_MS, true);
}

void FaceAuthBackend::onEnrollProgress(uint done, uint total, const QString &)
{
    if (!m_enrolling)
        return;
    m_capturesDone = int(done);
    m_capturesTotal = int(total);
    Q_EMIT capturesDoneChanged();
}

void FaceAuthBackend::cancelEnrollment()
{
    if (!m_enrolling)
        return;
    FaceAuthDaemon::call(QStringLiteral("CancelEnroll"), {}, this, [this](const Result &r) { track(r); });
}

void FaceAuthBackend::testAuth()
{
    if (m_testing || m_busy)
        return;
    m_testing = true;
    Q_EMIT testingChanged();
    setBusy(true);

    FaceAuthDaemon::call(
        QStringLiteral("Authenticate"), {m_username, QString(), uint(TEST_TIMEOUT_SECS)}, this,
        [this](const Result &r) {
            setBusy(false);
            track(r);
            m_testing = false;
            Q_EMIT testingChanged();
            if (r.ok)
                Q_EMIT authSucceeded();
            else
                Q_EMIT authFailed(r.unavailable ? QString() : r.errorMessage);
        },
        (TEST_TIMEOUT_SECS + 30) * 1000);
}

void FaceAuthBackend::clearLastError()
{
    setLastError(QString());
}
