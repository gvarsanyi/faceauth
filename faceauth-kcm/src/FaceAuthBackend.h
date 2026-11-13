// SPDX-License-Identifier: MIT OR Apache-2.0
#pragma once

#include <QObject>
#include <QVariantList>
#include <QtQmlIntegration>

namespace FaceAuthDaemon {
struct Result;
}

// QML-facing state and actions for the current user's face authentication.
// All daemon access is asynchronous over D-Bus.
class FaceAuthBackend : public QObject
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Instantiated by C++")

    // --- Model state ---
    Q_PROPERTY(bool enrolled READ enrolled NOTIFY modelChanged)
    // /dev/videoN of the enrolled camera; -1 if not enrolled or not connected.
    Q_PROPERTY(int cameraIndex READ cameraIndex NOTIFY modelChanged)
    // List of {index, name, suitability}.
    Q_PROPERTY(QVariantList cameras READ cameras NOTIFY camerasChanged)

    // --- Services: list of {name, enabled} for admin-allowed services ---
    Q_PROPERTY(QVariantList servicesList READ servicesList NOTIFY servicesChanged)

    // --- Enrollment ---
    Q_PROPERTY(bool enrolling READ enrolling NOTIFY enrollingChanged)
    Q_PROPERTY(int capturesDone READ capturesDone NOTIFY capturesDoneChanged)
    Q_PROPERTY(int capturesTotal READ capturesTotal NOTIFY capturesDoneChanged)

    // --- Test authentication ---
    Q_PROPERTY(bool testing READ testing NOTIFY testingChanged)

    // --- Daemon availability and async status ---
    Q_PROPERTY(bool daemonAvailable READ daemonAvailable NOTIFY daemonAvailableChanged)
    Q_PROPERTY(bool busy READ busy NOTIFY busyChanged)
    Q_PROPERTY(QString lastError READ lastError NOTIFY lastErrorChanged)

public:
    explicit FaceAuthBackend(QObject *parent = nullptr);

    bool enrolled() const { return m_batchCount > 0; }
    int cameraIndex() const { return m_cameraIndex; }
    QVariantList cameras() const { return m_cameras; }
    QVariantList servicesList() const { return m_servicesList; }
    bool enrolling() const { return m_enrolling; }
    int capturesDone() const { return m_capturesDone; }
    int capturesTotal() const { return m_capturesTotal; }
    bool testing() const { return m_testing; }
    bool daemonAvailable() const { return m_daemonAvailable; }
    bool busy() const { return m_busy; }
    QString lastError() const { return m_lastError; }

Q_SIGNALS:
    void modelChanged();
    void camerasChanged();
    void servicesChanged();
    void daemonAvailableChanged();
    void enrollingChanged();
    void capturesDoneChanged();
    void busyChanged();
    void lastErrorChanged();
    void testingChanged();

    // Emitted when an enrollment batch has been stored.
    void enrollmentSucceeded();
    // Emitted when testAuth() completes.
    void authSucceeded();
    void authFailed(const QString &message);

public Q_SLOTS:
    // Reload model, cameras, and services from the daemon.
    Q_INVOKABLE void refresh();
    // Enable or disable face authentication for an admin-allowed service.
    Q_INVOKABLE void setServiceEnabled(const QString &service, bool enabled);
    // Delete all enrolled face data.
    Q_INVOKABLE void clearAll();
    // Capture a new batch on /dev/video<cameraIndex>; -1 reuses the enrolled camera.
    Q_INVOKABLE void startEnrollment(int cameraIndex);
    Q_INVOKABLE void cancelEnrollment();
    // Run one authentication attempt; emits authSucceeded() or authFailed().
    Q_INVOKABLE void testAuth();
    Q_INVOKABLE void clearLastError();

private Q_SLOTS:
    void onEnrollProgress(uint done, uint total, const QString &status);
    void onDaemonUnregistered();

private:
    void loadModel();
    void loadCameras();
    void loadServices();
    void setBusy(bool b);
    void setLastError(const QString &err);
    void setEnrolling(bool e);
    // Track daemon reachability from a call result. Returns true if the
    // result is a daemon-level error that should be shown to the user.
    bool track(const FaceAuthDaemon::Result &r);
    // Report a failed operation unless it was a cancelled polkit prompt.
    void reportFailure(const FaceAuthDaemon::Result &r);

    QString m_username;
    int m_batchCount = 0;
    int m_cameraIndex = -1;
    QVariantList m_cameras;
    QVariantList m_servicesList;
    bool m_enrolling = false;
    int m_capturesDone = 0;
    int m_capturesTotal = 5;
    bool m_testing = false;
    bool m_daemonAvailable = true; // optimistic until the first reply
    bool m_busy = false;
    QString m_lastError;
};
