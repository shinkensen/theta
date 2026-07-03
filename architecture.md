





Essentially this will adopt a MoE type system architecture, deriving 
inspiration from Apple Inteligence and the combination of local models and 
cloud hosted ones. But with this, we will utilize a variety of cloud models 
such as free ones from nvidia 


I will be using Layer 1 as GPT OSS 120B on open router




explain this to me, i dont know rust. Im familiar with c++, java, python with my main language being typescript. I understand a bit of this due to the familiar syntax. so far this is what i absorbed


before defining each function (fn nameOfFunction (parameters) -> return type (similar to arrow function) { function definiton}). im not sure what 
the  :: are, i know that rust uses like an 'ownership' based system but im not quite sure what this is about, this reminds me of the scope 
declaration in c++ . I know that SystemInfo is just like a type/interface in typescript, also the variables/variable definition sytax is similar. 
the std is just the standard library. OHHH so like std::env::consts is like the consts defined in the env vars but the global env vars which 
define the os and additonal data regarding the system, and to_string() just makes them into a string. :: kind of reminds me of the dot operator, 
like SystemTime seems like a library with the now() just ouputing the time in seconds with the specification of the number of milliseconds since 
unix started and to interpret that as seconds and to throw the error as a string if there is an error with map_error, i want to know how map error 
actially works though? what is the  |e| ?. I think that Ok is just like return (like on Okay return this). I think that like &<variable name> is a 
way to utilize the parameter in the funciton, maybe something to do with the ownership design philosophy? I think that by default rust variables 
are immutable with you needing to specify their mutability by saying "mut" before the variable name like let mut var = ... and that Vec is just 
like vectors in c++ i believe, like essentially arrays. is pub like Public? as in like exportable? im not sure what cfg_attr(mobile,
tauri::mobile_entry_point) means but by just intuition, i think its refering to if ths was being ran on a mobile device