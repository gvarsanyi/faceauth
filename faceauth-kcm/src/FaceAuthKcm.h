#pragma once

#include <KQuickConfigModule>
#include "FaceAuthBackend.h"

class FaceAuthKcm : public KQuickConfigModule
{
    Q_OBJECT

    // Expose the backend to QML as kcm.backend.
    // KQuickConfigModule automatically injects 'kcm' as a context property,
    // so QML can access kcm.backend.enrolled, kcm.backend.setDisabled(), etc.
    Q_PROPERTY(FaceAuthBackend *backend READ backend CONSTANT)

public:
    FaceAuthKcm(QObject *parent, const KPluginMetaData &data);

    FaceAuthBackend *backend() const { return m_backend; }

private:
    FaceAuthBackend *m_backend;
};
