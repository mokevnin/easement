package web

import org.koin.core.Koin
import org.threeten.bp.LocalDate

class Handler(val koin: Koin, val day: LocalDate)
