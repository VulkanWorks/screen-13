use {
    super::Driver,
    gfx_hal::{device::Device, Backend},
    gfx_impl::Backend as _Backend,
    std::ops::{Deref, DerefMut},
};

pub struct Semaphore {
    driver: Driver,
    ptr: Option<<_Backend as Backend>::Semaphore>,
}

impl Semaphore {
    pub fn new(#[cfg(debug_assertions)] name: &str, driver: Driver) -> Self {
        let semaphore = {
            let device = driver.borrow();
            let ctor = || device.create_semaphore().unwrap();

            #[cfg(debug_assertions)]
            let mut semaphore = ctor();

            #[cfg(not(debug_assertions))]
            let semaphore = ctor();

            #[cfg(debug_assertions)]
            unsafe {
                device.set_semaphore_name(&mut semaphore, name);
            }

            semaphore
        };

        Self {
            driver,
            ptr: Some(semaphore),
        }
    }

    /// Sets a descriptive name for debugging which can be seen with API tracing tools such as RenderDoc.
    #[cfg(debug_assertions)]
    pub fn set_name(semaphore: &mut Self, name: &str) {
        let device = semaphore.driver.borrow();
        let ptr = semaphore.ptr.as_mut().unwrap();

        unsafe {
            device.set_semaphore_name(ptr, name);
        }
    }
}

impl AsMut<<_Backend as Backend>::Semaphore> for Semaphore {
    fn as_mut(&mut self) -> &mut <_Backend as Backend>::Semaphore {
        &mut *self
    }
}

impl AsRef<<_Backend as Backend>::Semaphore> for Semaphore {
    fn as_ref(&self) -> &<_Backend as Backend>::Semaphore {
        &*self
    }
}

impl Deref for Semaphore {
    type Target = <_Backend as Backend>::Semaphore;

    fn deref(&self) -> &Self::Target {
        self.ptr.as_ref().unwrap()
    }
}

impl DerefMut for Semaphore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ptr.as_mut().unwrap()
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        let device = self.driver.borrow();
        let ptr = self.ptr.take().unwrap();

        unsafe {
            device.destroy_semaphore(ptr);
        }
    }
}
